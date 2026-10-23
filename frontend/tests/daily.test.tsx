// @vitest-environment jsdom
import {afterEach,expect,it,vi} from 'vitest';
import {act,cleanup,fireEvent,render,screen} from '@testing-library/react';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
import {AttentionLedger} from '../src/features/daily/attention';
import {binding} from '../src/features/daily/commands';
import {DailyUse} from '../src/features/daily/DailyUse';
import type {AgentInstance} from '../src/desktop';
import type {ApiClient} from '../src/api';
import * as desktop from '../src/desktop';
const scope={environment_id:'e',project_id:'p',worktree_id:'w'};
const instance=(cursor:number,kind='attention_waiting',text=''):AgentInstance=>({id:'i',...scope,connection:'connected',stale:false,cursor,sessions:[{id:'s',assigned:true,activity:'Unknown'}],events:[{sequence:cursor,kind,text,session_id:'s'}]} as AgentInstance);
afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('native activation for a removed or disconnected worktree reports unavailable without changing selection',async()=>{
 let activate: ((target:{environment_id:string;project_id:string;worktree_id:string;instance_id:string;session_id:string;sequence:number})=>void)|undefined;
 vi.spyOn(desktop,'desktopMode').mockReturnValue(true);
 vi.spyOn(desktop,'onAttentionOpen').mockImplementation(callback=>{activate=callback;return Promise.resolve(()=>{});});
 vi.spyOn(desktop,'onAttentionUnavailable').mockResolvedValue(()=>{});
 vi.spyOn(desktop,'onCloseRequested').mockResolvedValue(()=>{});
 const select=vi.fn(),view=vi.fn();
 render(<QueryClientProvider client={new QueryClient()}><DailyUse api={{} as ApiClient} layout={{explorer_visible:true,explorer_width:312,collapsed:[]}} arrange={vi.fn()} selection={null} select={select} view={view} projects={[]} connected={[]} ready environments={[]} beforeClose={async()=>{}}/></QueryClientProvider>);
 await act(async()=>activate?.({...scope,instance_id:'i',session_id:'s',sequence:1}));
 expect(select).not.toHaveBeenCalled();expect(view).not.toHaveBeenCalled();
 expect(screen.getByRole('status').textContent).toContain('Цель уведомления недоступна');
});
it('does not notify initial/history/Unknown/disconnect; deduplicates scoped events and rate limits',()=>{
 const ledger=new AttentionLedger();expect(ledger.observe(scope,instance(1),0)).toEqual([]);
 expect(ledger.observe(scope,instance(2,'agent_message_chunk'),1)).toEqual([]);
 const waiting=ledger.observe(scope,instance(3),2);expect(waiting).toMatchObject([{...scope,instance_id:'i',session_id:'s',kind:'waiting',sequence:3}]);
 expect(ledger.observe(scope,instance(3),3)).toEqual([]);
 expect(ledger.observe(scope,instance(4),4)).toEqual([]);
 expect(ledger.observe(scope,{...instance(5),stale:true},20000)).toEqual([]);
 expect(ledger.observe(scope,instance(5),20001)).toEqual([]);
 expect(ledger.observe(scope,instance(6,'turn_end','cancelled'),20002)).toEqual([]);
 expect(ledger.observe(scope,instance(7,'turn_end','end_turn'),20003)[0].kind).toBe('completed');
 expect(ledger.observe({...scope,environment_id:'other'},instance(8),20004)).toEqual([]);
 expect(ledger.observe(scope,instance(9,'attention_error'),20005)[0].kind).toBe('error');
 expect(ledger.observe(scope,instance(10),20006)).toEqual([]); // 3/30s global cap
});
it('preserves standard terminal/editor keys, AltGr and composition',()=>{
 const base={ctrlKey:true,altKey:false,shiftKey:false,metaKey:false,isComposing:false,key:'c'};
 expect(binding(base)).toBeUndefined();expect(binding({...base,key:'p',shiftKey:true})).toBeUndefined();
 expect(binding({...base,altKey:true,key:'p'})).toBe('palette');
 expect(binding({...base,altKey:true,key:'p',isComposing:true})).toBeUndefined();
 expect(binding({...base,altKey:true,key:'p',getModifierState:()=>true})).toBeUndefined();
});
it('explicit launch seeds a new instance before its first fast permission event',()=>{
 const ledger=new AttentionLedger();ledger.begin(scope,'i');
 expect(ledger.observe(scope,instance(1),0)[0]).toMatchObject({instance_id:'i',kind:'waiting'});
});
it('palette searches, uses keyboard, shows unavailable context; preferences preserve existing layout',async()=>{
 Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.setAttribute('open','');}});
 Object.defineProperty(HTMLDialogElement.prototype,'close',{configurable:true,value:function(this:HTMLDialogElement){this.removeAttribute('open');}});
 const arrange=vi.fn(),view=vi.fn();const layout={explorer_visible:true,explorer_width:312,collapsed:['env:e'],terminals:[]};
 render(<QueryClientProvider client={new QueryClient()}><DailyUse api={{} as ApiClient} layout={layout} arrange={arrange} selection={null} select={vi.fn()} view={view} projects={[]} connected={[]} ready environments={[]} beforeClose={async()=>{}}/></QueryClientProvider>);
 const emulator=document.createElement('div');emulator.className='terminal-emulator';const terminalInput=document.createElement('textarea');emulator.append(terminalInput);document.body.append(emulator);
 fireEvent.keyDown(terminalInput,{key:'l',ctrlKey:true});expect(screen.queryByRole('dialog',{name:'Палитра команд'})).toBeNull();emulator.remove();
 fireEvent.keyDown(window,{key:'p',ctrlKey:true,altKey:true});
 expect(screen.getByRole('dialog',{name:'Палитра команд'})).toBeTruthy();
 const input=screen.getByRole('combobox');fireEvent.change(input,{target:{value:'Changes'}});
 expect(screen.getByRole('option').getAttribute('aria-disabled')).toBe('true');
 fireEvent.keyDown(input,{key:'Enter'});expect(view).not.toHaveBeenCalled();
 fireEvent.change(input,{target:{value:'Настройки приложения'}});fireEvent.keyDown(input,{key:'Enter'});
 await act(async()=>{await new Promise(r=>setTimeout(r,10));});
 fireEvent.change(screen.getByLabelText('Тема'),{target:{value:'light'}});
 expect(arrange.mock.calls[0][0]).toMatchObject({...layout,preferences:{light_theme:true}});
});
