// @vitest-environment jsdom
import {afterEach,expect,it,vi} from 'vitest';
import {act,cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import * as desktop from '../src/desktop';
import {MemoryDiagnostics} from '../src/features/diagnostics/MemoryDiagnostics';
import type {MemoryStatus} from '../src/features/diagnostics/types';
import type {BudgetStatus} from '../src/features/diagnostics/types';
import {MemoryBudgetNotice} from '../src/features/diagnostics/MemoryBudget';

afterEach(()=>{cleanup();vi.restoreAllMocks();});
const initial:MemoryStatus={enabled:false,interval_seconds:30,retained_bytes:0,max_retained_bytes:128*1024*1024,directory:'C:/diagnostics',export_path:null,error:null,last_sample:null};
it('records only on explicit start, exports a retained report and stops',async()=>{
  const invoke=vi.spyOn(desktop,'command').mockImplementation(async(name,args)=>{
    expect(name).toBe('memory_diagnostics');
    const action=(args as {action:string}).action;
    return {...initial,enabled:action!=='status'&&action!=='stop',retained_bytes:action==='status'?0:1000,export_path:action==='export'?'C:/diagnostics/report.jsonl':null};
  });
  render(<MemoryDiagnostics/>);
  await screen.findByText(/Запись выключена/);
  expect(invoke.mock.calls.map(c=>c[1])).toEqual([{action:'status'}]);
  expect((screen.getByRole('button',{name:'Экспортировать отчёт'}) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('button',{name:'Начать запись'}));
  await screen.findByRole('button',{name:'Остановить запись'});
  fireEvent.click(screen.getByRole('button',{name:'Экспортировать отчёт'}));
  await waitFor(()=>expect((screen.getByLabelText('Путь к отчёту памяти') as HTMLInputElement).value).toBe('C:/diagnostics/report.jsonl'));
  fireEvent.click(screen.getByRole('button',{name:'Остановить запись'}));
  await screen.findByRole('button',{name:'Начать запись'});
  expect(invoke.mock.calls.map(c=>(c[1] as {action:string}).action)).toEqual(['status','start','export','stop']);
});
it('surfaces unavailable WSL metrics and disk errors without displaying missing values as zero',async()=>{
  vi.spyOn(desktop,'command').mockResolvedValue({...initial,enabled:true,error:'Нет места для записи',last_sample:{record_type:'sample',schema_version:1,captured_unix_ms:1,collection_duration_ms:4,host:null,environments:[{environment_id:'e',distro:'Ubuntu',snapshot:null,error:'worker_memory_unavailable'}],errors:[]}});
  render(<MemoryDiagnostics/>);
  expect(await screen.findByText('Нет места для записи')).toBeTruthy();
  expect(await screen.findByText(/снимок недоступен/)).toBeTruthy();
});
it('saves a binary GiB budget for the exact environment and disables it locally',async()=>{
  const setting={environment_id:'env-ubuntu',worktree_id:'worktree-a',limit_bytes:null as number|null};
  let saved=setting;
  const invoke=vi.spyOn(desktop,'command').mockImplementation(async(_name,args)=>{
    const request=args as {action:string;budget?:typeof setting};
    if(request.budget)saved=request.budget;
    return {...initial,budget_settings:[saved],budgets:[{...saved,state:saved.limit_bytes==null?'disabled':'diagnostics_disabled',accounted_bytes:null,excess_bytes:null,peak_accounted_bytes:0,complete:false,missing_pss:0,missing_accounting:0,fallback_rss:false,warning_active:false}]};
  });
  render(<MemoryDiagnostics/>);
  await screen.findByText('Бюджет выключен');
  fireEvent.change(screen.getByLabelText('Размер мягкого бюджета'),{target:{value:'1.5'}});
  fireEvent.change(screen.getByLabelText('Единица бюджета'),{target:{value:'GiB'}});
  fireEvent.click(screen.getByRole('button',{name:'Сохранить бюджет'}));
  await screen.findByText('Контроль неактивен: диагностика выключена');
  expect(invoke.mock.calls.some(c=>JSON.stringify(c[1])===JSON.stringify({action:'set_budget',budget:{...setting,limit_bytes:1.5*1024**3}}))).toBe(true);
  fireEvent.click(screen.getByRole('button',{name:'Выключить бюджет'}));
  await screen.findByText('Бюджет выключен');
  expect(saved.limit_bytes).toBeNull();
});
it('does not describe incomplete accounting as budget compliance',async()=>{
  vi.spyOn(desktop,'command').mockResolvedValue({...initial,enabled:true,budgets:[{environment_id:'e',worktree_id:'w',limit_bytes:1024**3,state:'incomplete',accounted_bytes:100,excess_bytes:0,peak_accounted_bytes:200,complete:false,missing_pss:3,missing_accounting:1,fallback_rss:true,warning_active:false}]});
  render(<MemoryDiagnostics/>);
  expect(await screen.findByText('Учёт неполный: соблюдение бюджета неизвестно')).toBeTruthy();
  expect(screen.getByText(/используется RSS для части процессов/)).toBeTruthy();
});
it('shows one persistent app notice from collector events and clears it when disabled',async()=>{
  let receive:((budgets:BudgetStatus[])=>void)|undefined;
  vi.spyOn(desktop,'onMemoryBudgetStatus').mockImplementation(callback=>{receive=callback;return Promise.resolve(()=>{});});
  const invoke=vi.spyOn(desktop,'command').mockResolvedValue({...initial,budgets:[]});
  const open=vi.fn();render(<MemoryBudgetNotice open={open}/>);
  await waitFor(()=>expect(invoke).toHaveBeenCalledTimes(1));
  const b:BudgetStatus={environment_id:'e',worktree_id:'w',limit_bytes:1024**3,state:'exceeded',accounted_bytes:2*1024**3,excess_bytes:1024**3,peak_accounted_bytes:2*1024**3,complete:true,missing_pss:1,missing_accounting:0,fallback_rss:true,warning_active:true};
  await act(async()=>receive?.([b]));await act(async()=>receive?.([b]));
  expect(screen.getAllByRole('alert')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button',{name:/Мягкий бюджет памяти превышен/}));expect(open).toHaveBeenCalledOnce();
  await act(async()=>receive?.([{...b,state:'diagnostics_disabled',warning_active:false}]));
  expect(screen.queryByRole('alert')).toBeNull();
  expect(invoke.mock.calls.map(c=>c[1])).toEqual([{action:'status'}]);
});
