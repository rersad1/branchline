import { afterEach, expect, it, vi } from 'vitest';
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => true, invoke }));
import { ApiClient } from '../src/api';
import { command } from '../src/desktop';
afterEach(() => vi.restoreAllMocks());
it('explicit worktree refresh remains scoped and rejects a foreign response',async()=>{
 const api=new ApiClient('');invoke.mockResolvedValueOnce({environment_id:'env',project_id:'p',worktrees:[]});
 await api.worktrees('p',undefined,'env',true);
 expect(invoke).toHaveBeenLastCalledWith('list_worktrees',{scope:{environment_id:'env',project_id:'p'},refresh:true});
 invoke.mockResolvedValueOnce({environment_id:'other',project_id:'p',worktrees:[]});
 await expect(api.worktrees('p',undefined,'env',true)).rejects.toMatchObject({kind:'error'});
});

it('onboarding uses scoped IPC, returns native cancel and preserves domain errors', async () => {
  const api = new ApiClient('never-send');
  invoke.mockResolvedValueOnce(null);
  expect(await api.pickWindowsFolder()).toBeNull();
  invoke.mockResolvedValueOnce({ environment_id: 'env', path: '/', entries: [], next_offset: null });
  await api.directories('env', '/', 50);
  expect(invoke).toHaveBeenLastCalledWith('browse_directories', { scope: { environment_id: 'env' }, request: { path: '/', offset: 50 } });
  invoke.mockRejectedValueOnce({ code: 'duplicate_project', message: 'Already registered' });
  await expect(api.addProject('env', '/repo')).rejects.toMatchObject({ message: 'Already registered' });
  invoke.mockResolvedValueOnce({ environment_id: 'foreign' });
  await expect(api.addProject('env', '/repo')).rejects.toMatchObject({ kind: 'error' });
  invoke.mockResolvedValueOnce({ environment_id: 'foreign' });
  await expect(api.directories('env', '/')).rejects.toMatchObject({ kind: 'error' });
});

it('desktop sends only typed scope through IPC and rejects foreign identity', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch');
  invoke.mockResolvedValueOnce([]).mockResolvedValueOnce({ environment_id: 'other', project_id: 'p', worktrees: [] });
  const api = new ApiClient('must-never-cross-ipc');
  await api.projects(undefined, 'env');
  expect(invoke).toHaveBeenCalledWith('list_projects', { scope: { environment_id: 'env' } });
  await expect(api.worktrees('p', undefined, 'env')).rejects.toMatchObject({ kind: 'error' });
  expect(fetch).not.toHaveBeenCalled();
  expect(JSON.stringify(invoke.mock.calls)).not.toContain('must-never-cross-ipc');
});
it('cancellation discards a late IPC result and sanitizes runtime failure', async () => {
  let finish!: (value: unknown) => void;
  invoke.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const controller = new AbortController();
  const request = command('runtime_environment', undefined, controller.signal);
  controller.abort();
  await expect(request).rejects.toMatchObject({ name: 'AbortError' });
  finish({ id: 'late' });
  invoke.mockRejectedValueOnce({ code: 'disconnected', message: 'Worker disconnected; retry connection' });
  await expect(new ApiClient('').projects(undefined, 'env')).rejects.toMatchObject({ kind: 'disconnected', message: 'Worker disconnected; retry connection' });
});
