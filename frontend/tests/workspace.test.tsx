// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, render, waitFor, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
const closeState=vi.hoisted(()=>({beforeClose:null as null|(()=>Promise<unknown>)}));
vi.mock('../src/features/daily/DailyUse',()=>({DailyUse:(props:{beforeClose:()=>Promise<unknown>})=>{closeState.beforeClose=props.beforeClose;return null;}}));
vi.mock('../src/desktop', () => ({ desktopMode: () => true, onRuntimeState: async () => () => {} }));
import { Shell } from '../src/App';
import { ApiClient, type ExecutionEnvironment } from '../src/api';
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.forEach(c => c.clear()); clients.length = 0; vi.restoreAllMocks(); });
function mount() {
  const api = new ApiClient('unused');
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
  vi.spyOn(api, 'runtimeStates').mockResolvedValue([]);
  vi.spyOn(api, 'projects').mockResolvedValue([]);
  const save = vi.spyOn(api, 'saveWorkspace').mockResolvedValue();
  return { api, client, save, show: () => render(<QueryClientProvider client={client}><Shell api={api}/></QueryClientProvider>) };
}
it('retains empty environment content while background registry requests are pending',async()=>{
 const {api,client,show}=mount();
 vi.spyOn(api,'environment').mockResolvedValue({id:'windows',kind:'windows',distro:null});
 vi.mocked(api.runtimeStates).mockResolvedValue([{environment:{id:'ubuntu',kind:'wsl',distro:'Ubuntu'},state:'connected',error:null}]);
 vi.spyOn(api,'loadWorkspace').mockResolvedValue({version:1,environments:[],selection:null,layout:{explorer_visible:true,explorer_width:280,collapsed:[]}});
 show();await waitFor(()=>expect(screen.getAllByText('Нет зарегистрированных проектов')).toHaveLength(2));
 for(const id of ['windows','ubuntu']){
  let finish!:(value:[])=>void;
  vi.mocked(api.projects).mockImplementation((_signal,environment)=>environment===id?new Promise(resolve=>{finish=resolve;}):Promise.resolve([]));
  await act(async()=>{void client.invalidateQueries({queryKey:['projects',id]});await new Promise(resolve=>setTimeout(resolve,20));});
  expect(client.getQueryState(['projects',id])?.fetchStatus).toBe('fetching');
  expect(screen.getAllByText('Нет зарегистрированных проектов')).toHaveLength(2);
  await act(async()=>finish([]));
  await waitFor(()=>expect(client.getQueryState(['projects',id])?.fetchStatus).toBe('idle'));
  expect(screen.getAllByText('Нет зарегистрированных проектов')).toHaveLength(2);
 }
});
it('preserves Windows collapse layout while local identity is still loading', async () => {
  const { api, save, show } = mount();
  let finish!: (env: ExecutionEnvironment) => void;
  vi.spyOn(api, 'environment').mockReturnValue(new Promise(resolve => { finish = resolve; }));
  vi.spyOn(api, 'loadWorkspace').mockResolvedValue({ version: 1, environments: [], selection: null, layout: { explorer_visible: true, explorer_width: 320, collapsed: ['env:windows'] } });
  show();
  await screen.findByRole('heading', { name: 'Выберите worktree' });
  expect(screen.queryByLabelText('WSL distro')).toBeNull();
  expect(screen.getByRole('button', { name: 'Добавить проект' })).toBeTruthy();
  expect(save).not.toHaveBeenCalled();
  await act(async () => { finish({ id: 'windows', kind: 'windows', distro: null }); });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Windows' }).getAttribute('aria-expanded')).toBe('false'));
  expect(save).not.toHaveBeenCalled();
});
it('clears an obsolete environment selection after all reference identities are confirmed', async () => {
  const { api, save, show } = mount();
  vi.spyOn(api, 'environment').mockResolvedValue({ id: 'windows', kind: 'windows', distro: null });
  vi.spyOn(api, 'loadWorkspace').mockResolvedValue({ version: 1, environments: [], selection: { environment_id: 'obsolete', project_id: 'old', worktree_id: 'removed' }, layout: { explorer_visible: true, explorer_width: 280, collapsed: [] } });
  show();
  await waitFor(() => expect(save).toHaveBeenCalledWith(null, { explorer_visible: true, explorer_width: 280, collapsed: [] }));
  expect(screen.getByRole('heading', { name: 'Выберите worktree' })).toBeTruthy();
});
it('failed UI save blocks close until a subsequent save succeeds',async()=>{
 const {api,save,show}=mount();
 vi.spyOn(api,'environment').mockResolvedValue({id:'windows',kind:'windows',distro:null});
 vi.spyOn(api,'loadWorkspace').mockResolvedValue({version:1,environments:[],selection:null,layout:{explorer_visible:true,explorer_width:280,collapsed:[]}});
 save.mockRejectedValueOnce(new Error('disk write failed'));
 show();await screen.findByRole('heading',{name:'Выберите worktree'});
 const toggle=screen.getByRole('button',{name:'Скрыть Explorer'});
 const {fireEvent}=await import('@testing-library/react');fireEvent.click(toggle);
 await waitFor(()=>expect(save).toHaveBeenCalledTimes(1));
 await expect(closeState.beforeClose!()).rejects.toThrow('disk write failed');
 fireEvent.click(screen.getByRole('button',{name:'Показать Explorer'}));
 await waitFor(()=>expect(save).toHaveBeenCalledTimes(2));await expect(closeState.beforeClose!()).resolves.toBeUndefined();
});
