// @vitest-environment jsdom
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {act,cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {QueryClient,QueryClientProvider} from '@tanstack/react-query';
import * as desktop from '../src/desktop';
import * as drafts from '../src/features/changes/drafts';
import {Updates} from '../src/features/updates/Updates';
import {DailyUse} from '../src/features/daily/DailyUse';
import type {ApiClient} from '../src/api';
import type {UpdateStatus} from '../src/desktop';
const current:UpdateStatus={current_version:'0.2.0',phase:'current',version:null,downloaded:0,total:null,error:null};
const available:UpdateStatus={...current,phase:'available',version:'0.2.1'};
const downloaded:UpdateStatus={...available,phase:'ready',downloaded:100,total:100};
beforeEach(()=>{
  vi.spyOn(desktop,'onUpdateStatus').mockResolvedValue(()=>{});
  Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.setAttribute('open','');}});
  Object.defineProperty(HTMLDialogElement.prototype,'close',{configurable:true,value:function(this:HTMLDialogElement){this.removeAttribute('open');}});
});
afterEach(()=>{cleanup();vi.useRealTimers();vi.restoreAllMocks();});
it('automatically checks releases without downloading or restarting an active workspace',async()=>{
  vi.useFakeTimers();const install=vi.fn();
  const invoke=vi.spyOn(desktop,'command').mockImplementation(async(_name,args)=>(args as {action:string}).action==='check'?available:current);
  render(<Updates ready install={install}/>);
  await act(async()=>{await Promise.resolve();});
  await act(async()=>{await vi.advanceTimersByTimeAsync(8000);});
  expect(invoke.mock.calls.map(c=>c[1])).toEqual([{action:'status'},{action:'check'}]);
  expect(install).not.toHaveBeenCalled();
  expect(screen.getByRole('button',{name:'Обновления Branchline'}).title).toContain('0.2.1');
});
it('does not offer installation after a failed download or signature verification',async()=>{
  const install=vi.fn();
  vi.spyOn(desktop,'command').mockImplementation(async(_name,args)=>(args as {action:string}).action==='download'?{...available,phase:'error',error:'Подпись обновления не прошла проверку.'}:available);
  render(<Updates ready install={install}/>);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Обновления Branchline'}).title).toContain('0.2.1'));
  fireEvent.click(screen.getByRole('button',{name:'Обновления Branchline'}));
  fireEvent.click(screen.getByRole('button',{name:'Скачать обновление'}));
  expect((await screen.findByRole('alert')).textContent).toContain('Подпись');
  expect(screen.queryByRole('button',{name:'Сохранить и перезапустить'})).toBeNull();
  expect(install).not.toHaveBeenCalled();
});
it('flushes Changes and persisted layout before installing; a save failure leaves the installer untouched',async()=>{
  vi.spyOn(desktop,'desktopMode').mockReturnValue(true);
  vi.spyOn(desktop,'onAttentionOpen').mockResolvedValue(()=>{});
  vi.spyOn(desktop,'onAttentionUnavailable').mockResolvedValue(()=>{});
  vi.spyOn(desktop,'onCloseRequested').mockResolvedValue(()=>{});
  const order:string[]=[];
  vi.spyOn(drafts,'flushDrafts').mockImplementation(async()=>{order.push('files');});
  let saveFails=true;
  const beforeClose=vi.fn(async()=>{order.push('layout');if(saveFails)throw Error('Layout не сохранён');});
  const invoke=vi.spyOn(desktop,'command').mockImplementation(async(name,args)=>{
    if(name!=='updates')throw Error('unexpected command');
    if((args as {action:string}).action==='install'){order.push('installer');return {...downloaded,phase:'installing'};}
    return downloaded;
  });
  render(<QueryClientProvider client={new QueryClient()}><DailyUse api={{} as ApiClient} layout={{explorer_visible:true,explorer_width:300,collapsed:[]}} arrange={vi.fn()} selection={null} select={vi.fn()} view={vi.fn()} projects={[]} connected={[]} ready environments={[]} beforeClose={beforeClose}/></QueryClientProvider>);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Обновления Branchline'}).title).toContain('0.2.1'));
  fireEvent.click(screen.getByRole('button',{name:'Обновления Branchline'}));
  fireEvent.click(screen.getByRole('button',{name:'Сохранить и перезапустить'}));
  expect((await screen.findByRole('alert')).textContent).toContain('Layout не сохранён');
  expect(order).toEqual(['files','layout']);
  expect(invoke.mock.calls.some(c=>(c[1] as {action:string}).action==='install')).toBe(false);
  saveFails=false;order.length=0;
  fireEvent.click(screen.getByRole('button',{name:'Сохранить и перезапустить'}));
  await waitFor(()=>expect(order).toEqual(['files','layout','installer']));
});
