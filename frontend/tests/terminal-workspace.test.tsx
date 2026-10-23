// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { TerminalPane, TerminalWorkspace } from '../src/features/terminal/TerminalWorkspace';
import { ApiClient, type TerminalResponse, type Worktree } from '../src/api';
import type { TerminalLayout, Installation } from '../src/desktop';
import { useState } from 'react';
const dimensions=vi.hoisted(()=>({rows:24,cols:80,onData:(_data:string)=>{},onOsc:(_data:string):boolean=>false,onKey:(_event:KeyboardEvent):boolean=>true,refresh:()=>{}}));
const lifetime=vi.hoisted(()=>({created:0,disposed:0}));
vi.mock('@xterm/xterm',()=>({Terminal:class {options={};modes={bracketedPasteMode:true};hasSelection(){return false;}paste(data:string){dimensions.onData(data);}parser={registerOscHandler:vi.fn((_id:number,cb:(s:string)=>boolean)=>{dimensions.onOsc=cb;return{dispose(){}};})};constructor(){lifetime.created++;}get rows(){return dimensions.rows;}get cols(){return dimensions.cols;}loadAddon(){}open(){}onData(cb:(data:string)=>void){dimensions.onData=cb;return{dispose(){}};}refresh(){dimensions.refresh();}focus(){}scrollToBottom(){}attachCustomKeyEventHandler(cb:(event:KeyboardEvent)=>boolean){dimensions.onKey=cb;}attachCustomWheelEventHandler(){}write(_d:unknown,cb?:()=>void){cb?.();}dispose(){lifetime.disposed++;}}}));
vi.mock('@xterm/addon-fit',()=>({FitAddon:class{fit(){}}}));
const tree={environment_id:'env',project_id:'repo',id:'tree',is_available:true} as Worktree;
const result:TerminalResponse={environment_id:'env',project_id:'repo',worktree_id:'tree',sessions:[],session_id:null,data:[],cursor:0,lost:false};
afterEach(()=>{cleanup();vi.restoreAllMocks();vi.unstubAllGlobals();vi.useRealTimers();});
it('invalidates native Paste on lost output before a foreground TUI can receive stale Ctrl+V',async()=>{
 vi.useFakeTimers();vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 Object.defineProperty(navigator,'clipboard',{configurable:true,value:{readText:async()=> 'plain text'}});
 let deliver!:(value:TerminalResponse)=>void;
 const api=new ApiClient('');const call=vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>r.action==='read'?new Promise<TerminalResponse>(resolve=>{deliver=resolve;}):result);
 const pane=render(<TerminalPane api={api} tree={tree} connected session={{id:'s',project_id:'repo',worktree_id:'tree',cwd:'/start',shell:'Windows PowerShell',state:'running',exit_code:null,error:null,rows:24,cols:80}}/>);
 const paste=()=>dimensions.onKey(new KeyboardEvent('keydown',{ctrlKey:true,shiftKey:true,code:'KeyV'}));
 await act(async()=>{dimensions.onOsc('P;NativePaste=1');paste();});
 expect(call.mock.calls.filter(([,r])=>r.action==='input').map(([,r])=>(r as {data:string}).data)).toEqual(['\x16']);
 await act(async()=>{deliver({...result,lost:true});});
 await act(async()=>{paste();await Promise.resolve();});
 expect(call.mock.calls.filter(([,r])=>r.action==='input').map(([,r])=>(r as {data:string}).data)).toEqual(['\x16','plain text']);
 pane.unmount();
});
it('keeps reported cwd display-only, clears unknown and updates themes without recreating xterm',async()=>{
 vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const api=new ApiClient('');const call=vi.spyOn(api,'terminal').mockResolvedValue(result);
 const initial=lifetime.created;
 render(<TerminalPane api={api} tree={tree} connected session={{id:'s',project_id:'repo',worktree_id:'tree',cwd:'/start',shell:'Bash',state:'running',exit_code:null,error:null,rows:24,cols:80}}/>);
 expect(screen.getByLabelText('Текущая директория').textContent).toContain('неизвестна');
 await act(async()=>{dimensions.onOsc('P;Cwd='+encodeURIComponent('/outside/тест пробел'));});
 expect(screen.getByLabelText('Текущая директория').textContent).toBe('/outside/тест пробел');
 expect(screen.getByLabelText('Текущая директория').title).toContain('/start');
 await act(async()=>{document.documentElement.dataset.theme='light';});
 expect(lifetime.created).toBe(initial+1);
 expect(call.mock.calls.every(([env,r])=>env==='env'&&r.project_id==='repo'&&r.worktree_id==='tree'&&r.action==='read')).toBe(true);
 await act(async()=>{dimensions.onOsc('P;Cwd=relative');});
 expect(screen.getByLabelText('Текущая директория').textContent).toContain('неизвестна');
});
it('retains each visited VT screen and read cursor across tabs/splits; disposes only closed tabs',async()=>{
 vi.useFakeTimers();vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const initial={...lifetime};
 const api=new ApiClient('');
 let sessions=['shell','tui'].map(id=>({id,project_id:'repo',worktree_id:'tree',cwd:'/fixture',shell:id==='tui'?'opencode':'Bash',state:'running' as const,exit_code:null,error:null,rows:24,cols:80}));
 const call=vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>{
  if(r.action==='close')sessions=sessions.filter(s=>s.id!==r.session_id);
  return {...result,sessions,cursor:r.action==='read'?r.cursor+1:0};
 });
 const layout:TerminalLayout={environment_id:'env',project_id:'repo',worktree_id:'tree',tabs:[{id:'shell',title:'Shell'},{id:'tui',title:'OpenCode'}],active_id:'shell',split_id:null,vertical:false};
 let update!:(v:TerminalLayout)=>void;
 function Host(){const [value,save]=useState(layout);update=save;return <TerminalWorkspace api={api} tree={tree} connected compact layout={value} save={save} installations={[]}/>;}
 const mounted=render(<Host/>);await act(async()=>{});
 expect(lifetime.created-initial.created).toBe(1); // The unvisited hidden tab has no consumer yet.
 await act(async()=>fireEvent.click(screen.getByRole('tab',{name:'OpenCode'})));
 await act(async()=>vi.advanceTimersByTime(100));
 for(let n=0;n<3;n++){
  await act(async()=>fireEvent.click(screen.getByRole('tab',{name:'Shell'})));
  await act(async()=>fireEvent.click(screen.getByRole('tab',{name:'OpenCode'})));
 }
 await act(async()=>update({...layout,active_id:'tui',split_id:'shell'}));
 expect(screen.getAllByRole('region',{name:/^Terminal (shell|tui)$/})).toHaveLength(2);
 await act(async()=>update({...layout,active_id:'shell',split_id:null}));
 expect(screen.getAllByRole('region',{name:/^Terminal (shell|tui)$/})).toHaveLength(1);
 expect(lifetime.created-initial.created).toBe(2);expect(lifetime.disposed-initial.disposed).toBe(0);
 for(const id of ['shell','tui']){
  const reads=call.mock.calls.map(([,r])=>r).filter(r=>r.action==='read'&&r.session_id===id);
  expect(reads.filter(r=>r.action==='read'&&r.cursor===0)).toHaveLength(1);
 }
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Закрыть OpenCode'})));
 expect(lifetime.disposed-initial.disposed).toBe(1);
 mounted.unmount();expect(lifetime.disposed-initial.disposed).toBe(2);
});
it('creates mixed tabs only after choosing +, keeps exact harness identity and closes only the chosen tab',async()=>{
 vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const api=new ApiClient('');let sessions:TerminalResponse['sessions']=[];
 const call=vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>{
  if(r.action==='open'||r.action==='open_harness')sessions=[...sessions,{id:r.request_id,project_id:'repo',worktree_id:'tree',cwd:'/fixture',shell:r.action==='open'?'Bash':'opencode',state:'running',exit_code:null,error:null,rows:24,cols:80}];
  if(r.action==='close')sessions=sessions.filter(s=>s.id!==r.session_id);
  return {...result,sessions};
 });
 const installation:Installation={id:'exact-install',environment_id:'env',name:'opencode',version:'2.0.24',executable:'/tools/opencode',protocol:'terminal',capabilities:[],compatibility:'terminal',manual:true};
 let latest:TerminalLayout|undefined;
 function Host(){const [layout,setLayout]=useState<TerminalLayout>();return <TerminalWorkspace api={api} tree={tree} connected compact layout={layout} save={v=>{latest=v;setLayout(v);}} installations={[installation]}/>;}
 render(<Host/>);await act(async()=>{});
 expect(call.mock.calls.every(([,r])=>r.action==='list')).toBe(true);
 fireEvent.click(screen.getByRole('button',{name:'Новая вкладка'}));expect(sessions).toHaveLength(0);
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Терминал'})));
 const shell=latest!.tabs[0];expect(shell.launch).toEqual({kind:'shell'});
 fireEvent.click(screen.getByRole('button',{name:'Новая вкладка'}));await act(async()=>fireEvent.click(screen.getByRole('button',{name:/OpenCode · 2.0.24/})));
 expect(latest!.tabs).toHaveLength(2);expect(latest!.tabs[1].launch).toEqual({kind:'harness',installation_id:'exact-install'});
 expect(call.mock.calls.find(([,r])=>r.action==='open_harness')?.[1]).toMatchObject({installation_id:'exact-install',project_id:'repo',worktree_id:'tree'});
 const harness=latest!.tabs[1];fireEvent.click(screen.getByRole('tab',{name:/Терминал 1/}));
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:`Закрыть ${harness.title}`})));
 expect(call.mock.calls.filter(([,r])=>r.action==='close').map(([,r])=>(r as {session_id:string}).session_id)).toEqual([harness.id]);
 expect(latest!.active_id).toBe(shell.id);expect(sessions.map(s=>s.id)).toEqual([shell.id]);
});
it('restores legacy tabs without guessing a launch type or auto-starting a process',async()=>{
 vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const api=new ApiClient('');const call=vi.spyOn(api,'terminal').mockResolvedValue(result);
 const layout:TerminalLayout={environment_id:'env',project_id:'repo',worktree_id:'tree',tabs:[{id:'legacy',title:'Old harness'}],active_id:'legacy',split_id:null,vertical:false};
 render(<TerminalWorkspace api={api} tree={tree} connected compact layout={layout} save={vi.fn()} installations={[]}/>);await act(async()=>{});
 expect(screen.getByRole('button',{name:'Запустить заново…'})).toBeTruthy();
 expect(call.mock.calls.every(([,r])=>r.action==='list')).toBe(true);
});
it('wakes an idle read after input without a polling delay or overlapping reads',async()=>{
 vi.useFakeTimers();vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const api=new ApiClient('');let reads=0,pending=false,finish!:(r:TerminalResponse)=>void;
 vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>{
  if(r.action==='read'){expect(pending).toBe(false);reads++;pending=true;return new Promise<TerminalResponse>(resolve=>{finish=response=>{pending=false;resolve(response);};});}
  return result;
 });
 const pane=render(<TerminalPane api={api} tree={tree} connected session={{id:'s',project_id:'repo',worktree_id:'tree',cwd:'/fixture',shell:'opencode',state:'running',exit_code:null,error:null,rows:24,cols:80}}/>);
 expect(reads).toBe(1);await act(async()=>finish(result));
 await act(async()=>dimensions.onData('x'));expect(reads).toBe(2);
 await act(async()=>dimensions.onData('y'));expect(reads).toBe(2);
 pane.unmount();await act(async()=>finish(result));await act(async()=>vi.advanceTimersByTime(1000));expect(reads).toBe(2);
});
it('serializes resize and coalesces delayed measurements to the latest pane size',async()=>{
 let observed!:()=>void,finish!:(v:TerminalResponse)=>void;
 vi.stubGlobal('ResizeObserver',class{constructor(callback:()=>void){observed=callback;}observe(){}disconnect(){}});
 vi.spyOn(HTMLElement.prototype,'clientWidth','get').mockReturnValue(500);
 vi.spyOn(HTMLElement.prototype,'clientHeight','get').mockReturnValue(400);
 const api=new ApiClient('');
 const call=vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>r.action==='resize'?new Promise<TerminalResponse>(resolve=>{finish=resolve;}):result);
 render(<TerminalPane api={api} tree={tree} connected session={{id:'s',project_id:'repo',worktree_id:'tree',cwd:'/fixture',shell:'fixture',state:'running',exit_code:null,error:null,rows:24,cols:80}}/>);
 dimensions.rows=40;dimensions.cols=120;observed();
 dimensions.rows=50;dimensions.cols=130;observed();
 expect(call.mock.calls.filter(([,r])=>r.action==='resize')).toHaveLength(1);
 await act(async()=>{finish(result);});
 const sizes=call.mock.calls.filter(([,r])=>r.action==='resize');
 expect(sizes).toHaveLength(2);expect(sizes[1][1]).toMatchObject({rows:50,cols:130});
 await act(async()=>{finish(result);});dimensions.rows=24;dimensions.cols=80;
});
it('restores stopped tabs without launching any process',async()=>{
  vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
  const api=new ApiClient('');const call=vi.spyOn(api,'terminal').mockResolvedValue(result);
  const layout:TerminalLayout={environment_id:'env',project_id:'repo',worktree_id:'tree',tabs:[{id:'old-session',title:'My shell'}],active_id:'old-session',split_id:null,vertical:false};
  render(<TerminalWorkspace api={api} tree={tree} connected layout={layout} save={vi.fn()}/>);
  await act(async()=>{});
  expect(screen.getByText(/Layout восстановлен/)).toBeTruthy();
  expect(call.mock.calls.every(([,r])=>r.action==='list')).toBe(true);
});
it('persists launch intent once and ignores completion after worktree unmount',async()=>{
  let resolve!:(r:TerminalResponse)=>void;const api=new ApiClient('');
  const call=vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>r.action==='open'?new Promise<TerminalResponse>(done=>{resolve=done;}):result);
  const save=vi.fn();const mounted=render(<TerminalWorkspace api={api} tree={tree} connected layout={undefined} save={save}/>);
  await act(async()=>{});
  const button=screen.getByRole('button',{name:'Новый терминал'});fireEvent.click(button);fireEvent.click(button);
  expect(call.mock.calls.filter(([,r])=>r.action==='open')).toHaveLength(1);expect(save).toHaveBeenCalledTimes(1);
  expect(save.mock.calls[0][0].tabs).toHaveLength(1);mounted.unmount();
  await act(async()=>resolve(result));expect(save).toHaveBeenCalledTimes(1);
});
it('ignores an old list response after explicit launch',async()=>{
  vi.useFakeTimers();vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
  const api=new ApiClient('');let oldList!:(r:TerminalResponse)=>void;let lists=0;
  vi.spyOn(api,'terminal').mockImplementation(async(_env,r)=>{
    if(r.action==='list'){if(++lists===2)return new Promise<TerminalResponse>(done=>{oldList=done;});return result;}
    if(r.action==='open')return {...result,session_id:r.request_id,sessions:[{id:r.request_id,project_id:'repo',worktree_id:'tree',cwd:'/fixture',shell:'Bash',state:'running',exit_code:null,error:null,rows:24,cols:80}]};
    return result;
  });
  function Host(){const [layout,save]=useState<TerminalLayout>();return <TerminalWorkspace api={api} tree={tree} connected layout={layout} save={save}/>;}
  render(<Host/>);await act(async()=>{});await act(async()=>{vi.advanceTimersByTime(1000);});
  fireEvent.click(screen.getByRole('button',{name:'Новый терминал'}));await act(async()=>{});
  expect(screen.getByText('Bash · running')).toBeTruthy();
  await act(async()=>oldList(result));expect(screen.getByText('Bash · running')).toBeTruthy();
});

it('renumbers mixed tabs after close, drag/drop, keyboard move and create without restarting sessions',async()=>{
 vi.stubGlobal('ResizeObserver',class{observe(){}disconnect(){}});
 const api=new ApiClient('');const call=vi.spyOn(api,'terminal').mockResolvedValue(result);
 let latest:TerminalLayout={environment_id:'env',project_id:'repo',worktree_id:'tree',tabs:[
  {id:'one',title:'Терминал 1',launch:{kind:'shell'}},
  {id:'two',title:'OpenCode 2',launch:{kind:'harness',installation_id:'oc'}},
  {id:'three',title:'Терминал 3',launch:{kind:'shell'}}],active_id:'one',split_id:null,vertical:false};
 function Host(){const [layout,setLayout]=useState(latest);return <TerminalWorkspace api={api} tree={tree} connected layout={layout} save={v=>{latest=v;setLayout(v);}}/>;}
 render(<Host/>);await act(async()=>{});
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Закрыть OpenCode 2'})));
 expect(latest.tabs.map(t=>t.title)).toEqual(['Терминал 1','Терминал 2']);
 const transfer={effectAllowed:'',dropEffect:'',setData:vi.fn()};
 fireEvent.dragStart(screen.getByRole('tab',{name:/Терминал 2/}).parentElement!,{dataTransfer:transfer});
 fireEvent.drop(screen.getByRole('tab',{name:/Терминал 1/}).parentElement!,{dataTransfer:transfer});
 expect(latest.tabs.map(t=>t.id)).toEqual(['three','one']);expect(latest.active_id).toBe('one');
 expect(latest.tabs.map(t=>t.title)).toEqual(['Терминал 1','Терминал 2']);
 fireEvent.keyDown(screen.getByRole('tab',{name:/Терминал 2/}),{key:'ArrowLeft',altKey:true,shiftKey:true});
 expect(latest.tabs.map(t=>t.id)).toEqual(['one','three']);
 expect(call.mock.calls.every(([,r])=>r.action==='list')).toBe(true);
 await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Новый терминал'})));
 expect(latest.tabs.map(t=>t.title)).toEqual(['Терминал 1','Терминал 2','Терминал 3']);
 expect(call.mock.calls.filter(([,r])=>r.action==='open')).toHaveLength(1);
});
