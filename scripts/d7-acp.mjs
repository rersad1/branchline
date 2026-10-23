// Developer-only wire-contract probe. Does not enable a production descriptor.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, isAbsolute, join, relative, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const manifest = JSON.parse(readFileSync(new URL('./d7-candidates.json', import.meta.url), 'utf8'));
const FRAME = 65536;
const UPDATE_KINDS = new Set(['user_message_chunk', 'agent_message_chunk', 'agent_thought_chunk', 'tool_call', 'tool_call_update', 'plan', 'available_commands_update', 'current_mode_update', 'config_option_update']);
const fail = code => Object.assign(new Error(code), { code });
function jsonFile(path) {
  if (statSync(path).size > 1024 * 1024) throw fail('manifest_limit');
  return JSON.parse(readFileSync(path, 'utf8'));
}
function absoluteFile(path) {
  if (!isAbsolute(path)) throw fail('absolute_path_required');
  const canonical = realpathSync(path);
  if (!statSync(canonical).isFile()) throw fail('file_required');
  return canonical;
}
export function validatePackage(root, candidate) {
  if (!isAbsolute(root)) throw fail('absolute_path_required');
  root = realpathSync(root);
  const p = jsonFile(join(root, 'package.json'));
  if (p.name !== candidate.package || p.version !== candidate.version || p.bin?.[candidate.name] !== candidate.entry) {
    throw fail('package_pin_mismatch');
  }
  const entry = absoluteFile(join(root, candidate.entry));
  const inside = relative(root, entry);
  if (inside === '..' || inside.startsWith(`..${sep}`) || isAbsolute(inside)) throw fail('entry_outside_package');
  const dependencies = {};
  for (const [name, version] of Object.entries(candidate.dependencies)) {
    let directory = dirname(entry), found;
    // Resolve installed dependency manifests using Node's node_modules ancestor order.
    for (;;) {
      const path = join(directory, 'node_modules', ...name.split('/'), 'package.json');
      if (existsSync(path)) { found = jsonFile(path); break; }
      const parent = dirname(directory);
      if (parent === directory) break;
      directory = parent;
    }
    if (found?.name !== name || found.version !== version) throw fail('dependency_pin_mismatch');
    dependencies[name] = found.version;
  }
  return { entry, dependencies };
}
export class Wire {
  constructor(child, timeout = 12000) {
    this.child = child; this.timeout = timeout; this.next = 1; this.pending = new Map();
    this.buffer = Buffer.alloc(0); this.sessions = new Set(); this.updates = new Map();
    this.permissions = 0; this.failure = null; this.closed = false;
    child.stdin.on('error', () => this.rejectAll(fail('write_failed')));
    child.stdout.on('error', () => this.rejectAll(fail('read_failed')));
    child.stdout.on('data', data => {
      try {
        this.buffer = Buffer.concat([this.buffer, data]);
        let end;
        while ((end = this.buffer.indexOf(10)) !== -1) {
          if (end + 1 > FRAME) throw fail('frame_limit');
          const line = this.buffer.subarray(0, end);
          this.buffer = this.buffer.subarray(end + 1);
          this.message(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(line)));
        }
        if (this.buffer.length > FRAME) throw fail('frame_limit');
      } catch { this.rejectAll(fail('invalid_frame')); }
    });
    child.on('error', () => this.rejectAll(fail('spawn_failed')));
    child.on('close', () => { this.closed = true; this.rejectAll(fail('disconnected')); });
  }
  rejectAll(error) {
    this.failure ??= error;
    for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); }
    this.pending.clear();
  }
  send(message) {
    const data = Buffer.from(`${JSON.stringify(message)}\n`);
    if (data.length > FRAME) throw fail('frame_limit');
    this.child.stdin.write(data, error => { if (error) this.rejectAll(fail('write_failed')); });
  }
  request(method, params, timeout = this.timeout) {
    if (this.failure) return Promise.reject(this.failure);
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(fail('rpc_timeout')); }, timeout);
      this.pending.set(id, { resolve, reject, timer, method });
      try { this.send({ jsonrpc: '2.0', id, method, params }); }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
  }
  message(v) {
    if (!v || typeof v !== 'object' || Array.isArray(v) || v.jsonrpc !== '2.0') throw fail('invalid_frame');
    if (v.method) {
      if (v.method === 'session/request_permission' && v.id !== undefined) {
        this.permissions++;
        // A probe never approves a tool. Permissions are explicitly cancelled.
        this.send({ jsonrpc: '2.0', id: v.id, result: { outcome: { outcome: 'cancelled' } } });
      } else if (v.id !== undefined) {
        this.send({ jsonrpc: '2.0', id: v.id, error: { code: -32601, message: 'Client capability not supported' } });
      } else if (v.method === 'session/update' && this.sessions.has(v.params?.sessionId)) {
        const kind = v.params?.update?.sessionUpdate;
        if (typeof kind !== 'string' || kind.length > 64) throw fail('invalid_update');
        const key = UPDATE_KINDS.has(kind) ? kind : 'unknown_update';
        this.updates.set(key, (this.updates.get(key) ?? 0) + 1);
      }
      return;
    }
    const p = this.pending.get(v.id);
    if (!p) throw fail('unknown_response');
    clearTimeout(p.timer); this.pending.delete(v.id);
    if (v.error !== undefined) { p.reject(fail('rpc_rejected')); return; }
    if (!v.result || typeof v.result !== 'object' || Array.isArray(v.result)) { p.reject(fail('invalid_response')); return; }
    if (p.method === 'session/new' && typeof v.result.sessionId === 'string') this.sessions.add(v.result.sessionId);
    p.resolve(v.result);
  }
  async close() {
    this.rejectAll(fail('probe_closed'));
    if (this.closed) return;
    const closed = new Promise(resolve => this.child.once('close', resolve));
    this.child.stdin.end();
    // Supervisor owns its descendants. Never signal a PID discovered from provider output.
    this.child.kill('SIGTERM');
    let timer;
    try { await Promise.race([closed, new Promise((_, reject) => { timer = setTimeout(() => reject(fail('cleanup_timeout')), 5000); })]); }
    finally { clearTimeout(timer); }
  }
}
function runVersion(supervisor, node, entry, cwd, env) {
  const r = spawnSync(supervisor, ['--harness-stdio', String(process.pid), node, entry, '--version'], {
    cwd, env, shell: false, windowsHide: true, timeout: 5000, maxBuffer: 4096, stdio: ['ignore', 'pipe', 'ignore'],
  });
  if (r.error || r.status !== 0) throw fail('version_probe_failed');
  return r.stdout.toString('utf8').trim().split(/\s+/).map(s => s.replace(/^v/, '')).filter(s => /^\d+\.\d+\.\d+$/.test(s));
}
export async function probe(options) {
  const candidate = manifest.candidates.find(c => c.name === options.harness);
  if (!candidate) throw fail('unknown_harness');
  const { entry, dependencies } = validatePackage(options.package, candidate);
  const supervisor = absoluteFile(options.supervisor), node = absoluteFile(options.node);
  if (candidate.name === 'codex-acp' && process.env.CODEX_PATH) throw fail('codex_override_unverified');
  const runtime = spawnSync(node, ['--version'], { shell: false, timeout: 3000, maxBuffer: 1024, stdio: ['ignore', 'pipe', 'ignore'] });
  const match = runtime.stdout?.toString().trim().match(/^v(\d+)\.\d+\.\d+$/);
  if (runtime.error || runtime.status !== 0 || !match || Number(match[1]) < candidate.node_major_min) throw fail('node_version_unsupported');
  const cwd = mkdtempSync(join(tmpdir(), 'branchline-d7-'));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AWM_') && !key.startsWith('GIT_')));
  let wire;
  try {
    const git = spawnSync('git', ['init', '-b', 'main', cwd], { shell: false, env, stdio: 'ignore', timeout: 5000 });
    if (git.error || git.status !== 0) throw fail('git_unavailable');
    if (!runVersion(supervisor, node, entry, cwd, env).includes(candidate.version)) throw fail('runtime_pin_mismatch');
    const child = spawn(supervisor, ['--harness-stdio', String(process.pid), node, entry, ...candidate.argv], {
      cwd, env, shell: false, windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'],
    });
    wire = new Wire(child);
    const init = await wire.request('initialize', { protocolVersion: 1, clientCapabilities: {}, clientInfo: { name: 'branchline-d7-probe', version: '0.1.1' } });
    if (init.protocolVersion !== 1 || !init.agentCapabilities || typeof init.agentCapabilities !== 'object' || Array.isArray(init.agentCapabilities)) throw fail('protocol_incompatible');
    const session = await wire.request('session/new', { cwd, mcpServers: [] });
    if (typeof session.sessionId !== 'string' || !session.sessionId || session.sessionId.length > 256) throw fail('invalid_session');
    let turn = 'not_requested';
    if (options.turn) {
      const result = await wire.request('session/prompt', { sessionId: session.sessionId, prompt: [{ type: 'text', text: 'Reply exactly BRANCHLINE_D7_ACK. Do not use tools or modify any files.' }] }, 60000);
      if (!['end_turn', 'cancelled', 'max_tokens', 'max_turn_requests', 'refusal'].includes(result.stopReason)) throw fail('invalid_stop_reason');
      turn = result.stopReason;
    }
    return {
      task: candidate.task, harness: candidate.name, version: candidate.version, dependencies,
      platform: process.platform, arch: process.arch, node: runtime.stdout.toString().trim(),
      protocol: 1, status: 'wire_contract_observed', turn, permissions_cancelled: wire.permissions,
      updates: Object.fromEntries(wire.updates),
      config_selectors: Array.isArray(session.configOptions) ? session.configOptions.length : 0,
      full_d7_acceptance: false,
    };
  } finally {
    try { if (wire) await wire.close(); }
    finally { rmSync(cwd, { recursive: true, force: true }); }
  }
}
async function main() {
  const options = {};
  const args = process.argv.slice(2);
  for (let i = 0; i < args.length; i++) {
    const key = args[i];
    if (key === '--turn') options.turn = true;
    else if (['--harness', '--package', '--node', '--supervisor', '--output'].includes(key) && args[i + 1] && !args[i + 1].startsWith('--')) options[key.slice(2)] = args[++i];
    else throw fail('invalid_arguments');
  }
  if (!options.output || !isAbsolute(options.output) || existsSync(options.output)) throw fail('new_absolute_output_required');
  let result;
  try { result = await probe(options); }
  catch (error) { result = { status: 'blocked_or_failed', code: /^[a-z_]+$/.test(error.code ?? '') ? error.code : 'probe_failed', full_d7_acceptance: false }; process.exitCode = 1; }
  writeFileSync(options.output, `${JSON.stringify(result, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  console.log(JSON.stringify({ status: result.status, full_d7_acceptance: false }));
}
if (process.argv[1] && pathToFileURL(fileURLToPath(import.meta.url)).href === pathToFileURL(process.argv[1]).href) {
  main().catch(() => { console.error('D7 probe failed; verify arguments and output path.'); process.exitCode = 1; });
}
