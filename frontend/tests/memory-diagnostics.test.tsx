// @vitest-environment jsdom
import {afterEach,expect,it,vi} from 'vitest';
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import * as desktop from '../src/desktop';
import {MemoryDiagnostics} from '../src/features/diagnostics/MemoryDiagnostics';
import type {MemoryStatus} from '../src/features/diagnostics/types';

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
