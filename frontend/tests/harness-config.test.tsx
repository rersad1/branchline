// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
vi.mock('../src/features/terminal/TerminalWorkspace', () => ({ TerminalWorkspace: () => <div/> }));
import { HarnessPanel } from '../src/features/dashboard/HarnessPanel';
import type { ApiClient, Worktree } from '../src/api';
import type { AgentInstance, AgentResponse } from '../src/desktop';
afterEach(cleanup);
it('uses confirmed config values and inserts a command without sending or losing the draft', async () => {
  const config = { id: 'effort', name: 'Reasoning', current_value: 'low', options: [{ id: 'low', name: 'Low' }, { id: 'high', name: 'High' }] };
  const instance = { id: 'agent', installation_id: 'i', owned: true, protocol_version: 1, connection: 'connected', stale: false, capabilities: {},
    sessions: [{ id: 's', assigned: true, activity: 'Idle', modes: [], config_options: [config], commands: [{ name: 'mcp', description: 'MCP status' }] }], events: [], permissions: [], cursor: 1 } as unknown as AgentInstance;
  let finish!: (response: AgentResponse) => void;
  const agents = vi.fn(async (_env, request) => request.action === 'config'
    ? new Promise<AgentResponse>(resolve => { finish = resolve; }) : { instances: [instance] });
  const api = { harnesses: vi.fn(async () => ({ installations: [{ id: 'i', name: 'Fixture', protocol: 'acp', capabilities: [] }] })), agents } as unknown as ApiClient;
  const tree = { environment_id: 'windows', project_id: 'p', id: 't', is_available: true } as Worktree;
  render(<HarnessPanel api={api} tree={tree} connected/>);
  await act(async () => {});
  fireEvent.click(screen.getByText('Чат'));
  const menu = screen.getByLabelText('Действия агента').closest('details')!;
  menu.open = true;
  const select = screen.getByLabelText('Agent configuration Reasoning') as HTMLSelectElement;
  fireEvent.change(select, { target: { value: 'high' } });
  expect(agents.mock.calls.find(call => call[1].action === 'config')?.[1]).toMatchObject({
    project_id: 'p', worktree_id: 't', instance_id: 'agent', session_id: 's', config_id: 'effort', value: 'high',
  });
  expect(select.value).toBe('low');
  const confirmed = { ...instance, sessions: [{ ...instance.sessions[0], config_options: [{ ...config, current_value: 'high' }] }] };
  await act(async () => finish({ instances: [confirmed] } as AgentResponse));
  expect(select.value).toBe('high');
  const prompt = screen.getByLabelText('Agent prompt') as HTMLTextAreaElement;
  fireEvent.change(prompt, { target: { value: 'keep this draft' } });
  fireEvent.click(screen.getByRole('button', { name: '/mcp' }));
  expect(prompt.value).toBe('/mcp keep this draft');
  expect(agents.mock.calls.some(call => call[1].action === 'prompt')).toBe(false);
});
