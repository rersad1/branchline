import {useEffect,useRef,useState} from 'react';
import {command} from '../../desktop';
import type {MemorySnapshot,MemoryStatus} from './types';

const size=(bytes:number|null|undefined)=>bytes==null?'Недоступно':`${(bytes/1024/1024).toLocaleString('ru-RU',{maximumFractionDigits:1})} МиБ`;
function Inventory({title,snapshot}:{title:string;snapshot:MemorySnapshot}) {
  const linux=snapshot.platform==='linux',m=snapshot.system;
  const processes=[...snapshot.processes].sort((a,b)=>(b.pss_bytes??b.resident_bytes??0)-(a.pss_bytes??a.resident_bytes??0));
  const vms=processes.filter(p=>p.category==='wsl_vm');
  return <section className="memory-inventory"><h3>{title}</h3>
    <dl className="memory-summary">
      {linux?<><div><dt>Анонимная память Linux</dt><dd>{size(m.anonymous_bytes)}</dd></div><div><dt>Файловый кеш + buffers</dt><dd>{size(m.cached_bytes)}</dd></div><div><dt>Освобождаемый slab</dt><dd>{size(m.reclaimable_slab_bytes)}</dd></div><div><dt>Swap занят</dt><dd>{size(m.swap_total_bytes!=null&&m.swap_free_bytes!=null?m.swap_total_bytes-m.swap_free_bytes:null)}</dd></div></>:vms.map(p=><div key={p.pid}><dt>{p.name} · PID {p.pid}</dt><dd>{size(p.resident_bytes)}</dd></div>)}
      <div><dt>Доступно системе</dt><dd>{size(m.available_bytes)}</dd></div><div><dt>Терминалы / worktree с процессами</dt><dd>{snapshot.terminals.filter(t=>t.state==='running').length} / {new Set(snapshot.terminals.filter(t=>t.state==='running').map(t=>t.worktree_id)).size}</dd></div>
    </dl>
    {linux&&<p className="muted">PSS учитывает долю общей памяти процесса. Системные счётчики Linux могут включать другие WSL2-дистрибутивы.</p>}
    {!!snapshot.errors.length&&<p role="alert">Часть метрик недоступна: {snapshot.errors.join(', ')}</p>}
    <div className="memory-table"><table><thead><tr><th>Процесс / PID</th><th>RAM (RSS)</th><th>{linux?'PSS':'Private commit'}</th><th>Worktree</th></tr></thead><tbody>{processes.slice(0,12).map(p=><tr key={`${p.pid}:${p.start_id}`}><td>{p.name}<small>PID {p.pid} · {p.threads} потоков{p.metrics_error?' · часть метрик недоступна':''}</small></td><td>{size(p.resident_bytes)}</td><td>{size(linux?p.pss_bytes:p.private_bytes)}</td><td>{p.worktree_id?.slice(0,8)??'—'}</td></tr>)}</tbody></table></div>
    <p className="muted">Показаны 12 крупнейших процессов. В журнале: {snapshot.processes.length}, вне лимита: {snapshot.omitted_processes}.{!linux&&' Private commit — выделенная частная память, включая выгруженную; это не объём RAM.'}</p>
  </section>;
}

export function MemoryDiagnostics() {
  const [status,setStatus]=useState<MemoryStatus|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false);
  const alive=useRef(false),revision=useRef(0),mutating=useRef(false);
  useEffect(()=>{
    alive.current=true;let reading=false;
    const read=async()=>{if(reading||mutating.current)return;reading=true;const version=revision.current;try{const result=await command('memory_diagnostics',{action:'status'});if(alive.current&&version===revision.current)setStatus(result);}catch(e){if(alive.current)setError((e as Error).message);}finally{reading=false;}};
    void read();const timer=setInterval(()=>void read(),5000);
    return()=>{alive.current=false;clearInterval(timer);};
  },[]);
  async function act(action:'start'|'stop'|'export') {
    if(mutating.current)return;mutating.current=true;revision.current++;setBusy(true);setError('');
    try{const result=await command('memory_diagnostics',{action});if(alive.current)setStatus(result);}catch(e){if(alive.current)setError((e as Error).message);}finally{mutating.current=false;if(alive.current)setBusy(false);}
  }
  const sample=status?.last_sample;
  return <div className="memory-diagnostics">
    <p>Локальная запись расхода памяти каждые 30 секунд. Включение сохраняется после перезапуска приложения; запись идёт, пока приложение открыто.</p>
    <p className="muted">Сохраняются имена процессов, PID, память, потоки, время CPU и ID worktree. Содержимое файлов, терминалов, запросы, аргументы команд и ключи не записываются. WSL для мониторинга автоматически не запускается.</p>
    <div className="dialog-actions"><button disabled={busy||!status} onClick={()=>void act(status?.enabled?'stop':'start')}>{status?.enabled?'Остановить запись':'Начать запись'}</button><button disabled={busy||!status||(!sample&&!status.retained_bytes)} onClick={()=>void act('export')}>Экспортировать отчёт</button></div>
    <p role="status">{status?(status.enabled?'Запись включена':'Запись выключена'):'Загрузка диагностики…'}{status?` · Журнал ${size(status.retained_bytes)} / ${size(status.max_retained_bytes)}`:''}</p>
    {(error||status?.error)&&<p role="alert">{error||status?.error}</p>}
    {status?.export_path&&<label>Отчёт готов — отправьте этот файл<input aria-label="Путь к отчёту памяти" readOnly value={status.export_path} onFocus={e=>e.currentTarget.select()}/><button onClick={()=>void navigator.clipboard.writeText(status.export_path!).catch(()=>setError('Не удалось скопировать путь; выделите его в поле.'))}>Копировать путь к отчёту</button></label>}
    {sample?<><p>Последний снимок: {new Date(sample.captured_unix_ms).toLocaleString('ru-RU')} · сбор {sample.collection_duration_ms} мс.</p>{!!sample.errors.length&&<p role="alert">{sample.errors.join(', ')}</p>}{sample.host&&<Inventory title="Windows / приложение и WSL VM" snapshot={sample.host}/>} {sample.environments.map(e=><div key={e.environment_id}>{e.snapshot?<Inventory title={`WSL · ${e.distro??e.environment_id}`} snapshot={e.snapshot}/>:<p role="alert">WSL · {e.distro??e.environment_id}: снимок недоступен ({e.error}).</p>}</div>)}</>:<p className="muted">Начните запись и работайте как обычно. После дня работы экспортируйте отчёт. Первый снимок появится через несколько секунд.</p>}
    <p className="muted">RSS процессов содержит общие страницы. Vmmem и процессы внутри WSL показывают разные уровни одной памяти — складывать их нельзя. Старые записи автоматически заменяются при достижении 128 МиБ; экспортированные отчёты сохраняются отдельно.</p>
  </div>;
}
