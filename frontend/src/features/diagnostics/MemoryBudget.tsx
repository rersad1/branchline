import {useEffect,useState} from 'react';
import {command,onMemoryBudgetStatus} from '../../desktop';
import type {Worktree,ExecutionEnvironment} from '../../api';
import type {BudgetSetting,BudgetStatus,MemoryStatus} from './types';

export const size=(bytes:number|null|undefined)=>bytes==null?'Недоступно':`${(bytes/1024/1024).toLocaleString('ru-RU',{maximumFractionDigits:1})} МиБ`;
export const budgetState:Record<string,string>={disabled:'Бюджет выключен',diagnostics_disabled:'Контроль неактивен: диагностика выключена',awaiting_sample:'Контроль неактивен: ожидается свежий снимок',unavailable_or_unattributed:'Контроль неактивен: среда недоступна или нет процессов с доказанным владельцем',incomplete:'Учёт неполный: соблюдение бюджета неизвестно',observed_below_budget:'Наблюдаемое потребление ниже бюджета',exceeded:'Мягкий бюджет превышен',hysteresis:'Предупреждение до снижения расхода до 90% бюджета'};

export function MemoryBudgetNotice({open}:{open:()=>void}) {
  const [budgets,setBudgets]=useState<BudgetStatus[]>([]);
  useEffect(()=>{
    let active=true,revision=0,dispose:(()=>void)|undefined;
    void onMemoryBudgetStatus(b=>{revision++;if(active)setBudgets(b);}).then(fn=>{if(active)dispose=fn;else fn();}).catch(()=>{});
    const requested=revision;
    void command('memory_diagnostics',{action:'status'}).then(s=>{if(active&&revision===requested)setBudgets(s.budgets??[]);}).catch(()=>{});
    return()=>{active=false;dispose?.();};
  },[]);
  const warnings=budgets.filter(b=>b.warning_active);
  if(!warnings.length)return null;
  return <div className="daily-message memory-warning" role="alert"><button onClick={open}>Мягкий бюджет памяти превышен · {warnings.length} worktree. Открыть диагностику</button><small>Только предупреждение; процессы продолжают работу.</small></div>;
}

export function BudgetEditor({status,worktrees,environments,busy,save}:{status:MemoryStatus;worktrees:Worktree[];environments:ExecutionEnvironment[];busy:boolean;save:(budget:BudgetSetting)=>Promise<void>}) {
  const settings=status.budget_settings??[];
  const options=new Map<string,{environment_id:string;worktree_id:string;label:string}>();
  for(const t of worktrees)options.set(`${t.environment_id}:${t.id}`,{environment_id:t.environment_id,worktree_id:t.id,label:t.branch??t.id.slice(0,8)});
  for(const b of settings)if(!options.has(`${b.environment_id}:${b.worktree_id}`))options.set(`${b.environment_id}:${b.worktree_id}`,{...b,label:b.worktree_id.slice(0,8)});
  const sample=status.last_sample;
  const sources=[...(sample?.host&&sample.host_environment_id?[{id:sample.host_environment_id,snapshot:sample.host}]:[]),...(sample?.environments??[]).flatMap(e=>e.snapshot?[{id:e.environment_id,snapshot:e.snapshot}]:[])];
  for(const s of sources)for(const g of s.snapshot.groups??[])if(g.worktree_id&&!options.has(`${s.id}:${g.worktree_id}`))options.set(`${s.id}:${g.worktree_id}`,{environment_id:s.id,worktree_id:g.worktree_id,label:g.worktree_id.slice(0,8)});
  const [selected,setSelected]=useState(''),[value,setValue]=useState<string|null>(null),[unit,setUnit]=useState('MiB');
  const key=options.has(selected)?selected:options.keys().next().value??'';
  const target=options.get(key);
  const existing=settings.find(b=>target&&b.environment_id===target.environment_id&&b.worktree_id===target.worktree_id);
  const envName=(id:string)=>`${environments.find(e=>e.id===id)?.distro??(sample?.host_environment_id===id?'Windows':'Среда')} · ${id.slice(0,8)}`;
  // Remount the input when the target or its saved value changes, preserving drafts
  // across status refreshes for the same worktree.
  const inputKey=`${key}:${existing?.limit_bytes}`;
  const defaultValue=existing?.limit_bytes==null?'':String(existing.limit_bytes/1024/1024);
  const number=Number(value??defaultValue);
  const bytes=number*(unit==='GiB'?1024**3:1024**2);
  const valid=Number.isFinite(bytes)&&bytes>=1024**2&&bytes<=2**50;
  return <section className="memory-inventory"><h3>Мягкий бюджет worktree</h3>
    <p>Бюджет выключен по умолчанию и сохраняется только на этом компьютере. Это предупреждение, а не жёсткий лимит RAM: процессы и команды продолжают работу.</p>
    <p className="muted">Для каждого найденного процесса: PSS, при его отсутствии — RSS. RSS может завышать общие страницы. Суммы не включают swap. Если часть RSS тоже недоступна или инвентаризация неполная, соблюдение бюджета неизвестно. Общие сервисы, процессы без владельца и отдельно запущенные контейнеры могут не входить в бюджет.</p>
    <div className="memory-budget-form"><label>Среда / worktree<select aria-label="Среда / worktree для бюджета" disabled={busy||!options.size} value={key} onChange={e=>{setSelected(e.target.value);setValue(null);setUnit('MiB');}}>{[...options].map(([key,t])=><option key={key} value={key}>{envName(t.environment_id)} / {t.label}</option>)}</select></label>
      <label>Бюджет<input key={inputKey} aria-label="Размер мягкого бюджета" type="number" min="0.001" step="any" placeholder="Выключен" value={value??defaultValue} disabled={busy||!target} onChange={e=>setValue(e.target.value)}/></label>
      <label>Единица<select aria-label="Единица бюджета" value={unit} disabled={busy} onChange={e=>{setValue(value??defaultValue);setUnit(e.target.value);}}><option value="MiB">МиБ</option><option value="GiB">ГиБ</option></select></label>
      <button disabled={busy||!target||!valid} onClick={()=>target&&void save({environment_id:target.environment_id,worktree_id:target.worktree_id,limit_bytes:Math.round(bytes)}).then(()=>{setValue(null);setUnit('MiB');})}>Сохранить бюджет</button>
      <button disabled={busy||!target||existing?.limit_bytes==null} onClick={()=>target&&void save({environment_id:target.environment_id,worktree_id:target.worktree_id,limit_bytes:null}).then(()=>{setValue(null);setUnit('MiB');})}>Выключить бюджет</button>
    </div>
    {!options.size&&<p className="muted">Добавьте worktree или дождитесь снимка с принадлежащими ему терминалами.</p>}
    {(status.budgets??[]).map(b=><div className={`memory-budget-row ${b.warning_active?'memory-warning':''}`} key={`${b.environment_id}:${b.worktree_id}`}>
      <strong>{envName(b.environment_id)} / {options.get(`${b.environment_id}:${b.worktree_id}`)?.label??b.worktree_id.slice(0,8)}</strong>
      <p role={b.warning_active?'alert':'status'}>{budgetState[b.state]??b.state}</p>
      <p>Учтено {size(b.accounted_bytes)} · бюджет {b.limit_bytes==null?'выключен':size(b.limit_bytes)} · превышение {size(b.excess_bytes)} · пик сессии {size(b.peak_accounted_bytes)}</p>
      <small>Полнота найденных процессов: {b.complete?'доступен выбранный показатель для всех':'неполная'} · без PSS: {b.missing_pss} · без учёта: {b.missing_accounting}{b.fallback_rss?' · используется RSS для части процессов':''}. Внешние процессы могут быть вне учёта.</small>
    </div>)}
    <p className="muted">Проверка на существующем цикле 30 секунд. Одно событие превышения; новое предупреждение разрешается после полного замера ≤90% бюджета. Изменение бюджета сбрасывает подавление. При выключенной диагностике или недоступной среде контроль неактивен.</p>
  </section>;
}
