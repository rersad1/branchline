import {useEffect,useRef,useState} from 'react';
import {command} from '../../desktop';
import type {MemorySnapshot,MemoryStatus} from './types';
import type {BudgetSetting} from './types';
import type {ExecutionEnvironment,Worktree} from '../../api';
import {BudgetEditor,size} from './MemoryBudget';

const change=(bytes:number|null|undefined)=>bytes==null?'—':`${bytes>=0?'+':'−'}${size(Math.abs(bytes))}`;
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
    {snapshot.coverage?<p className="muted">Обнаружено: {snapshot.coverage.discovered_processes} · не удалось прочитать: {snapshot.coverage.unreadable_processes} · исключены из области Windows: {snapshot.coverage.excluded_processes} · инвентаризация {snapshot.coverage.inventory_complete?'полная':'неполная'}. Сводки до ограничения подробного списка.</p>:<p className="muted">Старая схема: полнота сводок недоступна.</p>}
    {linux&&<p className="muted">Давление памяти PSI за 10 с: some {m.pressure_some?`${m.pressure_some.avg10}%`:'недоступно'} · full {m.pressure_full?`${m.pressure_full.avg10}%`:'недоступно'}. Доля времени задержки задач из-за памяти.</p>}
    {!!snapshot.groups?.length&&<div className="memory-table"><table><thead><tr><th>Владелец / категория</th><th>Процессы / потоки</th><th>RSS / доступный PSS / swap</th><th>Учтено / Δ / пик сессии</th><th>Полнота метрик</th></tr></thead><tbody>{snapshot.groups.map(g=><tr key={`${g.scope}:${g.worktree_id}`}><td>{g.scope==='worktree'?`Worktree ${g.worktree_id?.slice(0,8)}`:g.scope==='shared_service'?'Общие сервисы':'Без установленного владельца'}<small>{g.categories.map(c=>`${c.category}: ${c.totals.processes} (${size(c.totals.accounted_bytes)})`).join(' · ')}</small></td><td>{g.totals.processes} / {g.totals.threads}</td><td>{size(g.totals.resident_bytes)} / {size(g.totals.pss_bytes)} / {size(g.totals.swap_bytes)}</td><td>{size(g.totals.accounted_bytes)}<small>Δ {change(g.accounted_delta_bytes)} · пик {size(g.peak_accounted_bytes)}</small></td><td>Без RSS {g.totals.missing_rss} · PSS {g.totals.missing_pss}<small>Без swap {g.totals.missing_swap} · CPU {g.totals.missing_cpu} · start {g.totals.missing_start}</small></td></tr>)}</tbody></table><p className="muted">Суммы содержат только доступные значения; нулевой доступный PSS при пропусках не означает нулевой расход. «Учтено» = PSS или RSS каждого процесса. Категории, роли JVM и признак контейнера — эвристики.</p></div>}
    <details><summary>Подробные процессы ({processes.length})</summary><div className="memory-table"><table><thead><tr><th>Процесс / PID</th><th>RAM (RSS) / Δ / пик</th><th>{linux?'PSS / Δ / пик':'Private commit'}</th><th>CPU / swap</th><th>Worktree</th></tr></thead><tbody>{processes.map(p=><tr key={p.lifetime_id??`${p.pid}:${p.start_id}`}><td>{p.name}<small>PID {p.pid} · {p.threads} потоков · {p.category} {p.role}{p.container?' · контейнер (эвристика)':''}{p.metrics_error?' · часть метрик недоступна':''}</small></td><td>{size(p.resident_bytes)}<small>{change(p.resident_delta_bytes)} · пик {size(p.peak_resident_bytes)}</small></td><td>{size(linux?p.pss_bytes:p.private_bytes)}{linux&&<small>{change(p.pss_delta_bytes)} · пик {size(p.peak_pss_bytes)}</small>}</td><td>{p.cpu_percent==null?'CPU недоступен':`CPU ${p.cpu_percent.toFixed(1)}%`}<small>Δ CPU {p.cpu_delta_ms??'—'} мс · swap {size(p.swap_bytes)}</small></td><td>{p.worktree_id?.slice(0,8)??(p.relation==='app_descendant'?'Общий сервис':'Без владельца')}</td></tr>)}</tbody></table></div></details>
    <p className="muted">Подробных процессов в журнале: {snapshot.processes.length}, вне лимита подробностей: {snapshot.omitted_processes}.{!linux&&' Private commit — выделенная частная память, включая выгруженную; это не объём RAM.'}</p>
  </section>;
}

export function MemoryDiagnostics({worktrees=[],environments=[]}:{worktrees?:Worktree[];environments?:ExecutionEnvironment[]}) {
  const [status,setStatus]=useState<MemoryStatus|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false);
  const alive=useRef(false),revision=useRef(0),mutating=useRef(false);
  useEffect(()=>{
    alive.current=true;let reading=false;
    const read=async()=>{if(reading||mutating.current)return;reading=true;const version=revision.current;try{const result=await command('memory_diagnostics',{action:'status'});if(alive.current&&version===revision.current)setStatus(result);}catch(e){if(alive.current)setError((e as Error).message);}finally{reading=false;}};
    void read();const timer=setInterval(()=>void read(),5000);
    return()=>{alive.current=false;clearInterval(timer);};
  },[]);
  async function act(action:'start'|'stop'|'export'|'set_budget',budget?:BudgetSetting) {
    if(mutating.current)return;mutating.current=true;revision.current++;setBusy(true);setError('');
    try{const result=await command('memory_diagnostics',budget?{action,budget}:{action});if(alive.current)setStatus(result);}catch(e){if(alive.current)setError((e as Error).message);}finally{mutating.current=false;if(alive.current)setBusy(false);}
  }
  const sample=status?.last_sample;
  return <div className="memory-diagnostics">
    <p>Локальная запись расхода памяти каждые 30 секунд. Включение сохраняется после перезапуска приложения; запись идёт, пока приложение открыто.</p>
    <p className="muted">Сохраняются имена процессов, PID и start ID, ID загрузки/запуска, память, потоки, CPU и ID worktree. Полные команды, аргументы, environment, секреты, пути проектов, содержимое терминалов и запросы не записываются. Только для Java в Linux локально читается до 16 КиБ аргументов: сохраняется одна роль из фиксированного списка, исходные данные отбрасываются. Cgroup читается для эвристики контейнера, пути не сохраняются. WSL для мониторинга автоматически не запускается.</p>
    <div className="dialog-actions"><button disabled={busy||!status} onClick={()=>void act(status?.enabled?'stop':'start')}>{status?.enabled?'Остановить запись':'Начать запись'}</button><button disabled={busy||!status||(!sample&&!status.retained_bytes)} onClick={()=>void act('export')}>Экспортировать отчёт</button></div>
    <p role="status">{status?(status.enabled?'Запись включена':'Запись выключена'):'Загрузка диагностики…'}{status?` · Журнал ${size(status.retained_bytes)} / ${size(status.max_retained_bytes)}`:''}</p>
    {(error||status?.error)&&<p role="alert">{error||status?.error}</p>}
    {status&&<BudgetEditor status={status} worktrees={worktrees} environments={environments} busy={busy} save={budget=>act('set_budget',budget)}/>}
    {status?.export_path&&<label>Отчёт готов — отправьте этот файл<input aria-label="Путь к отчёту памяти" readOnly value={status.export_path} onFocus={e=>e.currentTarget.select()}/><button onClick={()=>void navigator.clipboard.writeText(status.export_path!).catch(()=>setError('Не удалось скопировать путь; выделите его в поле.'))}>Копировать путь к отчёту</button></label>}
    {sample?<><p>Последний снимок: {new Date(sample.captured_unix_ms).toLocaleString('ru-RU')} · сбор {sample.collection_duration_ms} мс · размер {sample.snapshot_bytes==null?'недоступен':`${sample.snapshot_bytes.toLocaleString('ru-RU')} байт`}.</p>{!!sample.errors.length&&<p role="alert">{sample.errors.join(', ')}</p>}{sample.host&&<Inventory title="Windows / приложение и WSL VM" snapshot={sample.host}/>} {sample.environments.map(e=><div key={e.environment_id}>{e.snapshot?<Inventory title={`WSL · ${e.distro??e.environment_id}`} snapshot={e.snapshot}/>:<p role="alert">WSL · {e.distro??e.environment_id}: снимок недоступен ({e.error}).</p>}</div>)}<details><summary>События последнего снимка ({sample.events?.length??0})</summary><ul>{sample.events?.map((e,i)=><li key={i}>{e.kind} · {e.environment_id.slice(0,8)} / {e.worktree_id?.slice(0,8)??'—'} · {e.reason}</li>)}</ul><p className="muted">Вне лимита событий: {sample.omitted_events??0}. Исчезновение из ограниченной выборки не означает завершение процесса.</p></details></>:<p className="muted">Начните запись и работайте как обычно. Первый снимок появится через несколько секунд.</p>}
    <p className="muted">RSS процессов содержит общие страницы. Vmmem и процессы внутри WSL показывают разные уровни одной памяти — складывать их нельзя. Старые записи автоматически заменяются при достижении 128 МиБ; экспортированные отчёты сохраняются отдельно.</p>
  </div>;
}
