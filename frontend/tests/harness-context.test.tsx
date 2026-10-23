// @vitest-environment jsdom
import {afterEach,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,render,screen} from '@testing-library/react';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
vi.mock('../src/desktop',async importOriginal=>({...await importOriginal<typeof import('../src/desktop')>(),desktopMode:()=>true}));
vi.mock('../src/features/terminal/TerminalWorkspace',()=>({TerminalWorkspace:()=> <section>Terminal fixture</section>}));
vi.mock('../src/features/dashboard/HarnessPanel',()=>({HarnessPanel:({tree}:{tree:{id:string}})=><section data-testid="harness-context">{tree.id}<input aria-label="Draft fixture" defaultValue=""/></section>}));
vi.mock('../src/features/changes/Changes',()=>({Changes:()=> <section>Changes fixture</section>}));
vi.mock('../src/features/dashboard/ProviderCheck',()=>({ProviderCheck:()=> <aside>Diagnostics</aside>}));
import {Dashboard} from '../src/features/dashboard/Dashboard';
import {ApiClient,type Worktree,type Project} from '../src/api';
afterEach(cleanup);
it('retains visited worktree panels while exposing only the selected context',()=>{
 const client=new QueryClient({defaultOptions:{queries:{retry:false,staleTime:Infinity}}});
 const api=new ApiClient('unused');
 const tree=(id:string)=>({id,project_id:'p',environment_id:'e',path:'/fixture/'+id,branch:'main',is_available:true,is_main:false,head:null,is_bare:false,is_detached:false,is_locked:false,lock_reason:null,is_prunable:false,prune_reason:null,availability_error:null,git_status:{staged:0,unstaged:0,untracked:0,conflicts:0,files:[],last_successful_refresh_at:new Date().toISOString(),error:null}} as Worktree);
 const snapshot={environment_id:'e',project_id:'p',worktrees:[tree('first'),tree('second')],error:null,last_successful_refresh_at:new Date().toISOString()};
 client.setQueryData(['worktrees','e','p'],snapshot);vi.spyOn(api,'worktrees').mockResolvedValue(snapshot);
 const project={id:'p',environment_id:'e',name:'fixture',root_path:'/fixture',git_common_dir:'/fixture/.git'} as Project;
 const view=(id:string)=><QueryClientProvider client={client}><Dashboard api={api} projects={[project]} selection={{environment_id:'e',project_id:'p',worktree_id:id}}/></QueryClientProvider>;
 const r=render(view('first'));expect(screen.getAllByTestId('harness-context')).toHaveLength(1);
 const first=screen.getByTestId('harness-context');
 r.rerender(view('second'));expect(screen.getAllByTestId('harness-context')).toHaveLength(2);
 expect(first.closest('[hidden]')).toBeTruthy();expect(screen.getAllByTestId('harness-context')[1].closest('[hidden]')).toBeNull();
 r.rerender(view('first'));expect(screen.getAllByTestId('harness-context')[0]).toBe(first);expect(first.closest('[hidden]')).toBeNull();
 r.unmount();client.clear();
});

it('retains the active worktree draft across views, mounts terminals lazily and preserves drafts between worktrees',()=>{
 const client=new QueryClient({defaultOptions:{queries:{retry:false,staleTime:Infinity}}});
 const api=new ApiClient('unused');
 const trees=['first','second'].map(id=>({id,project_id:'p',environment_id:'e',path:'/fixture/'+id,branch:'main',is_available:true,is_main:false,head:null,is_bare:false,is_detached:false,is_locked:false,lock_reason:null,is_prunable:false,prune_reason:null,availability_error:null,git_status:{staged:0,unstaged:0,untracked:0,conflicts:0,files:[],last_successful_refresh_at:new Date().toISOString(),error:null}} as Worktree));
 const snapshot={environment_id:'e',project_id:'p',worktrees:trees,error:null,last_successful_refresh_at:new Date().toISOString()};
 client.setQueryData(['worktrees','e','p'],snapshot);vi.spyOn(api,'worktrees').mockResolvedValue(snapshot);
 const project={id:'p',environment_id:'e',name:'fixture',root_path:'/fixture'} as Project;
 const view=(tab:'overview'|'changes'|'terminal',id='first')=><QueryClientProvider client={client}><Dashboard api={api} projects={[project]} selection={{environment_id:'e',project_id:'p',worktree_id:id}} view={tab} onView={()=>{}}/></QueryClientProvider>;
 const r=render(view('overview'));
 expect(screen.queryByText('Terminal fixture')).toBeNull();expect(screen.queryByText('Changes fixture')).toBeNull();
 const draft=screen.getByLabelText('Draft fixture');fireEvent.change(draft,{target:{value:'Keep this request'}});
 r.rerender(view('changes'));expect(draft.closest('[hidden]')).toBeTruthy();expect(screen.queryByRole('textbox')).toBeNull();
 r.rerender(view('terminal'));expect(screen.getByText('Changes fixture').closest('[hidden]')).toBeTruthy();expect(screen.getByLabelText('Draft fixture')).toBe(draft);expect(screen.queryByText('Terminal fixture')).toBeNull();
 r.rerender(view('overview'));expect(screen.getByLabelText('Draft fixture')).toBe(draft);expect((draft as HTMLInputElement).value).toBe('Keep this request');
 r.rerender(view('overview','second'));expect((screen.getByRole('textbox') as HTMLInputElement).value).toBe('');expect(draft.closest('[hidden]')).toBeTruthy();
 r.rerender(view('overview','first'));expect(screen.getByRole('textbox')).toBe(draft);expect((draft as HTMLInputElement).value).toBe('Keep this request');
 expect(screen.queryByText('Terminal fixture')).toBeNull();
 r.unmount();client.clear();
});

