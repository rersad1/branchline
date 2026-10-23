import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { manifest, validatePackage, Wire } from '../d7-acp.mjs';

function fixturePackage(candidate) {
  const root = mkdtempSync(join(tmpdir(), 'branchline-d7-test-'));
  writeFileSync(join(root, 'package.json'), JSON.stringify({ name: candidate.package, version: candidate.version, bin: { [candidate.name]: candidate.entry } }));
  mkdirSync(dirname(join(root, candidate.entry)), { recursive: true }); writeFileSync(join(root, candidate.entry), '');
  return root;
}
test('all four candidates pin package, entry, protocol and bridge dependency; none claim a verified platform', () => {
  assert.deepEqual(manifest.candidates.map(c => c.task), ['D7.1', 'D7.2', 'D7.3', 'D7.4']);
  assert.equal(manifest.protocol_version, 1);
  for (const candidate of manifest.candidates) {
    assert.match(candidate.version, /^\d+\.\d+\.\d+$/);
    assert.deepEqual(candidate.verified_platforms, []);
    assert.ok(!candidate.argv.some(a => /yolo|dangerous|bypass/.test(a)));
  }
  assert.equal(manifest.candidates[2].dependencies['@openai/codex'], '0.160.1');
  assert.equal(manifest.candidates[3].dependencies['@anthropic-ai/claude-agent-sdk'], '0.3.300');
});
test('package/version and paired dependency are checked without running package code', () => {
  const c = manifest.candidates[2], root = fixturePackage(c);
  try {
    assert.throws(() => validatePackage(root, c), { code: 'dependency_pin_mismatch' });
    const dep = join(root, 'node_modules', '@openai', 'codex'); mkdirSync(dep, { recursive: true });
    writeFileSync(join(dep, 'package.json'), JSON.stringify({ name: '@openai/codex', version: '0.160.1' }));
    assert.equal(validatePackage(root, c).dependencies['@openai/codex'], '0.160.1');
    const shadow = join(root, 'dist', 'node_modules', '@openai', 'codex'); mkdirSync(shadow, {recursive:true});
    writeFileSync(join(shadow, 'package.json'), JSON.stringify({name:'@openai/codex',version:'0.0.0'}));
    assert.throws(() => validatePackage(root, c), {code:'dependency_pin_mismatch'});
    rmSync(join(root, 'dist', 'node_modules'), {recursive:true});
    writeFileSync(join(dep, 'package.json'), JSON.stringify({ name: '@openai/codex', version: '0.162.0' }));
    assert.throws(() => validatePackage(root, c), { code: 'dependency_pin_mismatch' });
    assert.throws(() => validatePackage(root, { ...c, version: '9.9.9' }), { code: 'package_pin_mismatch' });
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test('entry cannot escape the selected package through a symlink', { skip: process.platform === 'win32' }, () => {
  const c = manifest.candidates[0], root = fixturePackage(c), external = mkdtempSync(join(tmpdir(), 'branchline-d7-other-'));
  try {
    writeFileSync(join(external, 'entry.js'), ''); rmSync(join(root, c.entry)); symlinkSync(join(external, 'entry.js'), join(root, c.entry));
    assert.throws(() => validatePackage(root, c), { code: 'entry_outside_package' });
  } finally { rmSync(root, { recursive: true, force: true }); rmSync(external, { recursive: true, force: true }); }
});
function start(scenario, timeout = 500) {
  // In-memory peer: exercises framing/state without claiming native process evidence.
  const child = new EventEmitter(); child.stdout = new PassThrough();
  const send = value => child.stdout.write(`${JSON.stringify(value)}\r\n`);
  let buffered = '', promptId;
  child.stdin = new Writable({ write(bytes, _encoding, done) {
    buffered += bytes.toString();
    let end;
    while ((end = buffered.indexOf('\n')) !== -1) {
      const v = JSON.parse(buffered.slice(0, end)); buffered = buffered.slice(end + 1);
      if (scenario === 'timeout') continue;
      if (scenario === 'overflow') { child.stdout.write('x'.repeat(65537)); continue; }
      if (scenario === 'invalid') { child.stdout.write('SECRET_PROVIDER_STDERR\n'); continue; }
      if (v.method === 'initialize') {
        const reply = JSON.stringify({ jsonrpc: '2.0', id: v.id, result: { protocolVersion: 1, agentCapabilities: {} } });
        child.stdout.write(reply.slice(0, 8)); setTimeout(() => child.stdout.write(`${reply.slice(8)}\r\n`), 5);
      } else if (v.method === 'session/new') send({ jsonrpc: '2.0', id: v.id, result: { sessionId: 's' } });
      else if (v.method === 'session/prompt') {
        promptId = v.id;
        send({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: 'foreign', update: { sessionUpdate: 'tool_call' } } });
        send({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: 's', update: { sessionUpdate: 'agent_message_chunk' } } });
        send({ jsonrpc: '2.0', id: 'permission', method: 'session/request_permission', params: { sessionId: 's' } });
      } else if (v.id === 'permission') {
        assert.equal(v.result.outcome.outcome, 'cancelled'); send({ jsonrpc: '2.0', id: promptId, result: { stopReason: 'end_turn' } });
      } else if (v.method === 'reject') send({ jsonrpc: '2.0', id: v.id, error: { message: 'SECRET_PROVIDER_ERROR' } });
    }
    done();
  } });
  child.kill = () => { queueMicrotask(() => child.emit('close', 0)); return true; };
  return new Wire(child, timeout);
}
test('fragmented CRLF frames, exact session update mapping and denied permission flow', async () => {
  const wire = start('normal');
  try {
    assert.equal((await wire.request('initialize', {})).protocolVersion, 1);
    await wire.request('session/new', {});
    assert.equal((await wire.request('session/prompt', { sessionId: 's' })).stopReason, 'end_turn');
    assert.equal(wire.permissions, 1); assert.equal(wire.updates.get('agent_message_chunk'), 1);
    assert.equal(wire.updates.has('tool_call'), false);
    assert.equal(wire.buffer.includes(Buffer.from('never recorded')), false);
    await assert.rejects(wire.request('reject', {}), error => error.code === 'rpc_rejected' && !error.message.includes('SECRET'));
  } finally { await wire.close(); }
  assert.equal(wire.closed, true);
});
test('unknown provider update kinds use one fixed counter without persisting their text', async () => {
  const wire = start('normal');
  try {
    wire.sessions.add('s');
    for (let i = 0; i < 1000; i++) wire.message({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: 's', update: { sessionUpdate: `SECRET_KIND_${i}` } } });
    assert.deepEqual(Object.fromEntries(wire.updates), { unknown_update: 1000 });
  } finally { await wire.close(); }
});
for (const scenario of ['overflow', 'invalid', 'timeout']) {
  test(`${scenario} rejects within a bound and closes the fixture transport`, async () => {
    const wire = start(scenario, scenario === 'timeout' ? 50 : 500);
    try { await assert.rejects(wire.request('initialize', {}), error => ['invalid_frame', 'rpc_timeout'].includes(error.code) && !error.message.includes('SECRET')); }
    finally { await wire.close(); }
    assert.equal(wire.closed, true);
  });
}
test('real child transport smoke (not provider or platform compatibility)', async context => {
  const available = spawnSync(process.execPath, ['-e', 'process.stdout.write("child_probe")'], { encoding: 'utf8' });
  if (available.error?.code === 'EPERM' || available.error?.code === 'EACCES') {
    context.skip('Execution environment denies child processes'); return;
  }
  assert.ifError(available.error);
  assert.equal(available.stdout, 'child_probe');
  const wire = new Wire(spawn(process.execPath, [fileURLToPath(new URL('./d7-wire-fixture.mjs', import.meta.url)), 'normal'], { stdio: ['pipe', 'pipe', 'ignore'], shell: false }));
  try { assert.equal((await wire.request('initialize', {})).protocolVersion, 1); }
  finally { await wire.close(); }
});
