// @vitest-environment jsdom
import {afterEach,expect,it,vi} from 'vitest';
import {act,cleanup,fireEvent,render,screen,within} from '@testing-library/react';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
import {Dashboard} from '../src/features/dashboard/Dashboard';
import {ApiClient,type Worktree,type Project,type Snapshot} from '../src/api';
import type {Layout} from '../src/desktop';
vi.mock('@tauri-apps/api/core',()=>({isTauri:()=>true,invoke:vi.fn()}));
vi.mock('../src/features/dashboard/HarnessPanel',()=>({HarnessPanel:()=>null}));
vi.mock('../src/features/dashboard/ProviderCheck',()=>({ProviderCheck:()=>null}));
vi.mock('../src/features/terminal/TerminalWorkspace',()=>({TerminalWorkspace:()=>null}));
const clients:QueryClient[]=[];
afterEach(()=>{cleanup();clients.forEach(c=>c.clear());clients.length=0;vi.restoreAllMocks();});
const project={id:'repo',environment_id:'ubuntu',name:'Fixture',root_path:'/fixture'} as Project;
const tree=(id:string,missing=false):Worktree=>({id,environment_id:'ubuntu',project_id:'repo',path:'/fixture/'+id,branch:id,head:'abc',is_main:id==='main',is_bare:false,is_locked:false,is_detached:false,is_prunable:missing,is_available:!missing,lock_reason:null,prune_reason:missing?'missing path':null,availability_error:null,git_status:{staged:0,unstaged:0,untracked:0,conflicts:0,files:[],last_successful_refresh_at:new Date().toISOString(),error:null}});
function mount(){
 const main=tree('main'),missing=tree('missing',true),snapshot:Snapshot={environment_id:'ubuntu',project_id:'repo',worktrees:[main,missing],last_successful_refresh_at:new Date().toISOString(),error:null};
 const client=new QueryClient({defaultOptions:{queries:{staleTime:Infinity,retry:false}}});clients.push(client);client.setQueryData(['worktrees','ubuntu','repo'],snapshot);
 const api=new ApiClient('');vi.spyOn(api,'worktrees').mockResolvedValue(snapshot);
 const select=vi.fn(),save=vi.fn(),removed=vi.fn(),unregister=vi.fn().mockResolvedValue(undefined);
 const layout:Layout={explorer_visible:true,explorer_width:280,collapsed:[],terminals:['main','missing'].map(id=>({environment_id:'ubuntu',project_id:'repo',worktree_id:id,tabs:[],active_id:null,split_id:null,vertical:false}))};
 HTMLDialogElement.prototype.showModal=function(){this.setAttribute('open','');};
 render(<QueryClientProvider client={client}><Dashboard api={api} projects={[project]} selection={{environment_id:'ubuntu',project_id:'repo',worktree_id:'main'}} onSelection={select} layout={layout} onLayout={save} onWorktreeRemoved={removed} remove={unregister}/></QueryClientProvider>);
 return {api,client,main,missing,snapshot,select,save,removed,unregister};
}
it('uses the same scoped actions for right-click and visible ellipsis; refresh bypasses the cache',async()=>{
 const f=mount();const row=screen.getByRole('button',{name:/missing.*Prunable/});
 fireEvent.contextMenu(row,{clientX:100,clientY:150});
 const menu=screen.getByRole('group',{name:'Действия с worktree missing'});
 expect(within(menu).getByText('Удалить недоступный worktree…')).toBeTruthy();expect(f.select).not.toHaveBeenCalled();
 await act(async()=>fireEvent.click(within(menu).getByRole('button',{name:'Обновить'})));
 expect(f.api.worktrees).toHaveBeenCalledWith('repo',undefined,'ubuntu',true);
 fireEvent.click(screen.getByRole('button',{name:'Действия с worktree missing'}));
 expect(within(screen.getByRole('group')).getByText('Удалить недоступный worktree…')).toBeTruthy();
 fireEvent.keyDown(document,{key:'Escape'});expect(screen.queryByRole('group')).toBeNull();
});
it('removes a missing entry through confirmation and preserves a different selected worktree and its layout',async()=>{
 const f=mount();const call=vi.spyOn(f.api,'removeWorktree').mockResolvedValue({environment_id:'ubuntu',project_id:'repo',worktree_id:'missing',git_removed:true,path_exists:false,snapshot:{...f.snapshot,worktrees:[f.main]},warning:null});
 fireEvent.contextMenu(screen.getByRole('button',{name:/missing.*Prunable/}));
 fireEvent.click(screen.getByText('Удалить недоступный worktree…'));
 expect(screen.getByRole('dialog',{name:'Удалить недоступный worktree'})).toBeTruthy();
 fireEvent.change(screen.getByLabelText('Подтвердите полный путь'),{target:{value:'/fixture/missing'}});fireEvent.click(screen.getByRole('checkbox'));
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Удалить worktree'})));
 expect(call).toHaveBeenCalledWith('ubuntu',{project_id:'repo',worktree_id:'missing',confirm_path:'/fixture/missing',external_sessions_checked:true,missing_only:true});
 expect(f.select).not.toHaveBeenCalled();expect(f.removed).toHaveBeenCalledWith({environment_id:'ubuntu',project_id:'repo',worktree_id:'main'},expect.objectContaining({terminals:[expect.objectContaining({worktree_id:'main'})]}));
});
it('protects Primary worktree and exposes project registration removal without touching Git worktrees',async()=>{
 const f=mount();fireEvent.click(screen.getByRole('button',{name:'Действия с worktree main'}));
 expect((screen.getByRole('button',{name:'Удалить worktree…'}) as HTMLButtonElement).disabled).toBe(true);
 fireEvent.keyDown(document,{key:'Escape'});
 fireEvent.contextMenu(screen.getByRole('button',{name:'Fixture'}));
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Убрать проект из списка'})));
 expect(f.unregister).toHaveBeenCalledWith(project);expect(f.select).toHaveBeenCalledWith(null);
});
