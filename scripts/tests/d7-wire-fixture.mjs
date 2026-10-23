import { createInterface } from 'node:readline';
const scenario = process.argv[2];
const send = v => process.stdout.write(`${JSON.stringify(v)}\r\n`);
createInterface({ input: process.stdin }).on('line', line => {
  const v = JSON.parse(line);
  if (scenario === 'timeout') return;
  if (scenario === 'overflow') { process.stdout.write('x'.repeat(65537)); return; }
  if (scenario === 'invalid') { process.stdout.write('SECRET_PROVIDER_STDERR\n'); return; }
  if (v.method === 'initialize') {
    const wire = JSON.stringify({ jsonrpc: '2.0', id: v.id, result: { protocolVersion: 1, agentCapabilities: {} } });
    process.stdout.write(wire.slice(0, 8)); setTimeout(() => process.stdout.write(`${wire.slice(8)}\r\n`), 5);
  } else if (v.method === 'session/new') {
    send({ jsonrpc: '2.0', id: v.id, result: { sessionId: 's' } });
  } else if (v.method === 'session/prompt') {
    send({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: 'foreign', update: { sessionUpdate: 'tool_call' } } });
    send({ jsonrpc: '2.0', method: 'session/update', params: { sessionId: 's', update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'never recorded' } } } });
    send({ jsonrpc: '2.0', id: 'permission', method: 'session/request_permission', params: { sessionId: 's', options: [{ optionId: 'allow', name: 'Allow' }] } });
    globalThis.promptId = v.id;
  } else if (v.id === 'permission') {
    if (v.result?.outcome?.outcome !== 'cancelled') process.exit(1);
    send({ jsonrpc: '2.0', id: globalThis.promptId, result: { stopReason: 'end_turn' } });
  } else if (v.method === 'reject') {
    send({ jsonrpc: '2.0', id: v.id, error: { code: -32000, message: 'SECRET_PROVIDER_ERROR' } });
  }
});
