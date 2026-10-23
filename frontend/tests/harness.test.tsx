// @vitest-environment jsdom
import { afterEach,expect,it,vi } from 'vitest';
import { act,cleanup,fireEvent,render,screen } from '@testing-library/react';
vi.mock('../src/features/terminal/TerminalWorkspace',()=>({TerminalWorkspace:()=> <div>Terminal workspace</div>}));
import { HarnessPanel } from '../src/features/dashboard/HarnessPanel';
import { ApiClient,type Worktree } from '../src/api';
import type { AgentInstance, AgentResponse } from '../src/desktop';
const tree={environment_id:'windows',project_id:'p',id:'t',is_available:true} as Worktree;
afterEach(cleanup);
it('D7 npm candidate requires explicit test launch and passes a separate Node path',async()=>{
 HTMLDialogElement.prototype.showModal=function(){this.setAttribute('open','');};
 HTMLDialogElement.prototype.close=function(){this.removeAttribute('open');};
 const installation={id:'npm',name:'qwen',version:'0.25.2',protocol:'terminal',capabilities:['D7 candidate validation'],npm:{node:'C:/tools/node.exe',package:'C:/tools/qwen',entry:'C:/tools/qwen/cli-entry.js',name:'qwen'}};
 const api={harnesses:vi.fn(async()=>({installations:[installation]})),agents:vi.fn(async()=>({instances:[]}))} as unknown as ApiClient;
 render(<HarnessPanel api={api} tree={tree} connected/>);await act(async()=>{});
 expect((api.agents as ReturnType<typeof vi.fn>).mock.calls.every(c=>c[1].action==='list')).toBe(true);
 fireEvent.click(screen.getByRole('button',{name:'Чат'}));
 fireEvent.click(screen.getByRole('button',{name:'Тест ACP D7'}));await act(async()=>{});
 expect((api.agents as ReturnType<typeof vi.fn>).mock.calls.find(c=>c[1].action==='launch')?.[1]).toMatchObject({candidate:true,installation_id:'npm'});
 fireEvent.click(screen.getByLabelText('Действия агента'));fireEvent.click(screen.getByRole('button',{name:'Настроить harness'}));
 fireEvent.change(screen.getByLabelText('Manual harness executable'),{target:{value:'C:/tools/qwen'}});
 fireEvent.change(screen.getByLabelText('Harness Node executable'),{target:{value:'C:/tools/node.exe'}});
 fireEvent.click(screen.getByRole('button',{name:'Добавить'}));await act(async()=>{});
 expect((api.harnesses as ReturnType<typeof vi.fn>).mock.calls.at(-1)?.[1]).toMatchObject({action:'add',executable:'C:/tools/qwen',node:'C:/tools/node.exe'});
});
it('late discovery cannot update a different worktree; discovery does not launch',async()=>{
 let finish!:(v:unknown)=>void;
 const api={harnesses:vi.fn(()=>new Promise(r=>{finish=r;})),agents:vi.fn(async()=>({instances:[]}))} as unknown as ApiClient;
 const v=render(<HarnessPanel api={api} tree={tree} connected/>);v.unmount();
 const next={...api,harnesses:vi.fn(async()=>({installations:[]}))} as unknown as ApiClient;
 render(<HarnessPanel api={next} tree={{...tree,id:'other'}} connected/>);
 await act(async()=>{finish({installations:[{id:'old',name:'SECRET_OLD_CONTEXT',executable:'/old',capabilities:[]}]});});
 expect(screen.queryByText(/SECRET_OLD_CONTEXT/)).toBeNull();
 expect((api.agents as ReturnType<typeof vi.fn>).mock.calls.every(call=>call[1].action==='list')).toBe(true);
});

it('preserves a failed prompt and does not erase a newer draft after delayed success',async()=>{
 const instance={id:'agent',installation_id:'i',owned:true,protocol_version:1,connection:'connected',stale:false,capabilities:{},sessions:[{id:'s',assigned:true,activity:'Idle',modes:[]}],events:[],permissions:[],cursor:1} as unknown as AgentInstance;
 const response={instances:[instance]} as AgentResponse;
 let send!:(value:AgentResponse)=>void;let rejectSend!:(error:Error)=>void;
 const api={harnesses:vi.fn(async()=>({installations:[{id:'i',name:'Fixture',protocol:'acp',capabilities:[]}]})),agents:vi.fn(async(_env,request)=>request.action==='prompt'?new Promise<AgentResponse>((resolve,reject)=>{send=resolve;rejectSend=reject;}):response)} as unknown as ApiClient;
 render(<HarnessPanel api={api} tree={tree} connected/>);await act(async()=>{});
 fireEvent.click(screen.getByRole('button',{name:'Чат'}));
 const prompt=screen.getByRole('textbox',{name:'Agent prompt'}) as HTMLTextAreaElement;
 fireEvent.change(prompt,{target:{value:'Do not lose this request'}});fireEvent.submit(prompt.closest('form')!);
 expect(prompt.value).toBe('Do not lose this request');
 await act(async()=>rejectSend(new Error('Connection lost')));expect(prompt.value).toBe('Do not lose this request');
 // Resume the normal list poll to confirm the connection again, without retrying the prompt.
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,850));});
 fireEvent.submit(prompt.closest('form')!);fireEvent.change(prompt,{target:{value:'The next request'}});
 await act(async()=>send(response));expect(prompt.value).toBe('The next request');
 fireEvent.submit(prompt.closest('form')!);await act(async()=>send(response));expect(prompt.value).toBe('');
 expect((api.agents as ReturnType<typeof vi.fn>).mock.calls.filter(call=>call[1].action==='prompt')).toHaveLength(3);
});
it('launch is explicit and carries installation identity, never port/cwd/shell string',async()=>{
 const api={harnesses:vi.fn(async()=>({installations:[{id:'i',name:'opencode',version:'1.19.0',executable:'C:/opencode.exe',protocol:'acp',compatibility:'pinned',capabilities:[]}]})),agents:vi.fn(async()=>({instances:[]}))} as unknown as ApiClient;
 render(<HarnessPanel api={api} tree={tree} connected/>);
 await act(async()=>{});fireEvent.click(screen.getByText('Чат'));fireEvent.click(screen.getByText('Запустить'));
 await act(async()=>{});
 const request=(api.agents as ReturnType<typeof vi.fn>).mock.calls.find(call=>call[1].action==='launch')?.[1];
 expect(request).toMatchObject({project_id:'p',worktree_id:'t',installation_id:'i',action:'launch'});expect(request).not.toHaveProperty('port');expect(request).not.toHaveProperty('cwd');
});
it('defaults to one unified native workspace, hides diagnostics and retains it across Chat mode',async()=>{
 const save=vi.fn();
 const terminal=vi.fn(async(_environment:string,_request:unknown)=>({}));
 const api={harnesses:vi.fn(async()=>({installations:[{id:'i',name:'opencode',version:'1.19.0',executable:'C:/opencode.exe',protocol:'acp',capabilities:[]}]})),agents:vi.fn(async()=>({instances:[]})),terminal} as unknown as ApiClient;
 render(<HarnessPanel api={api} tree={tree} connected saveLayout={save} diagnostics={<div>HEAD private details</div>}/>);
 await act(async()=>{});
 expect(terminal).not.toHaveBeenCalled();
 expect(screen.queryByText('HEAD private details')).toBeNull();
 expect(screen.queryByLabelText('Manual harness executable')).toBeNull();
 expect(screen.queryByLabelText('External OpenCode port')).toBeNull();
 expect(save).not.toHaveBeenCalled();
 expect(screen.getByText('Terminal workspace')).toBeTruthy();
 const native=screen.getByText('Terminal workspace');
 fireEvent.click(screen.getByText('Чат'));
 expect(native.parentElement?.hidden).toBe(true);
 fireEvent.click(screen.getByText('Терминал'));
 expect(screen.getByText('Terminal workspace')).toBe(native);
 expect(terminal).not.toHaveBeenCalled();
});

