import { useEffect, useRef, useState } from 'react';
import { useQueries } from '@tanstack/react-query';
import type { ApiClient, Project, Worktree, ExecutionEnvironment } from '../../api';
import { worktreeCacheKey } from '../../api';
import { command, defaultPreferences, desktopMode, onCloseRequested, onAttentionOpen, onAttentionUnavailable, type Layout, type Selection } from '../../desktop';
import { AgentDialog } from '../dashboard/AgentDialog';
import { bindings, binding, shortcut, buttonCommands, commandButton, type Command } from './commands';
import { AttentionLedger, attentionLabel, type Attention } from './attention';
import { Icon } from '../../components/ui/icon';
import { allDrafts, discardDrafts, draftRevision, flushDrafts, saveDraft, subscribeDrafts, requestDraft } from '../changes/drafts';
import { useSyncExternalStore } from 'react';
import { ActionMenu } from '../../components/ui/action-menu';
import { MemoryDiagnostics } from '../diagnostics/MemoryDiagnostics';

export function DailyUse({api,layout,arrange,selection,select,view,projects,connected,ready,environments,beforeClose,onAddProject}: {
  api:ApiClient;layout:Layout;arrange:(layout:Layout)=>void;selection:Selection|null;select:(selection:Selection)=>void;
  view:(view:'overview'|'changes'|'terminal')=>void;projects:Project[];connected:string[];ready:boolean;environments:ExecutionEnvironment[];beforeClose:()=>Promise<unknown>;
  onAddProject?:()=>void;
}) {
  const preferences = {...defaultPreferences,...layout.preferences};
  const [dialog,setDialog] = useState<'palette'|'preferences'|'shortcuts'|'close'|'memory'|null>(null);
  const [search,setSearch] = useState(''),[index,setIndex] = useState(0),[error,setError] = useState('');
  const [attention,setAttention] = useState<Attention[]>([]),[target,setTarget] = useState<Attention|null>(null);
  const ledger = useRef(new AttentionLedger());
  const queries = useQueries({queries:projects.map(p=>({queryKey:worktreeCacheKey(p.environment_id,p.id),enabled:connected.includes(p.environment_id),queryFn:({signal}:{signal:AbortSignal})=>api.worktrees(p.id,signal,p.environment_id)}))});
  const trees = queries.flatMap((q,i)=> !q.isError && !q.data?.error && connected.includes(projects[i].environment_id) ? q.data?.worktrees.filter(t=>t.is_available&&!t.is_bare)??[]:[]);
  const current = trees.find(t=>t.environment_id===selection?.environment_id&&t.project_id===selection.project_id&&t.id===selection.worktree_id);
  const state = useRef({preferences,trees,ready,beforeClose});state.current={preferences,trees,ready,beforeClose};
  useSyncExternalStore(subscribeDrafts, draftRevision);
  const failedDrafts = allDrafts().filter(d => d.status === 'error' || d.status === 'conflict');
  const closing = useRef(false);
  const closeNow = (discard = false) => {
    if (closing.current) return; closing.current = true;
    void (async () => {
      if (discard) discardDrafts(); else await flushDrafts();
      await state.current.beforeClose(); await command('close_application', undefined);
    })().catch(error => { setError(`Закрытие отменено: ${error.message}`); setDialog('close'); }).finally(() => { closing.current = false; });
  };
  const save = (patch:Partial<typeof preferences>)=>arrange({...layout,preferences:{...preferences,...patch}});
  useEffect(()=>{
    const launched=(event:Event)=>{const target=(event as CustomEvent).detail;if(state.current.trees.some(t=>t.environment_id===target.environment_id&&t.project_id===target.project_id&&t.id===target.worktree_id))ledger.current.begin(target,target.instance_id);};
    window.addEventListener('agent-launched',launched);return()=>window.removeEventListener('agent-launched',launched);
  },[]);
  useEffect(()=>{ document.documentElement.dataset.theme=preferences.light_theme?'light':'dark'; },[preferences.light_theme]);
  useEffect(()=>{
    if(!desktopMode())return;
    const subscription=onCloseRequested(()=>{
      if(state.current.ready&&state.current.preferences.close_without_confirmation)closeNow();
      else setDialog('close');
    });
    return()=>{void subscription.then(off=>off());};
  },[]);
  function openAttention(item:Attention){
    if(!state.current.ready||!state.current.trees.some(t=>t.environment_id===item.environment_id&&t.project_id===item.project_id&&t.id===item.worktree_id)){
      setTarget(null);setError('Цель уведомления недоступна: среда отключена или worktree удалён.');return;
    }
    setTarget(item);select({environment_id:item.environment_id,project_id:item.project_id,worktree_id:item.worktree_id});view('overview');setDialog(null);
  }
  const openRef=useRef(openAttention);openRef.current=openAttention;
  // Monitor registered scopes, independent of the selected panel. Never launch or adopt.
  const scopes=trees.map(t=>({environment_id:t.environment_id,project_id:t.project_id,worktree_id:t.id}));
  const scopesKey=JSON.stringify(scopes);
  useEffect(()=>{
    if(!desktopMode())return;
    const abort=new AbortController();let timer:ReturnType<typeof setTimeout>;
    const poll=async()=>{
      for(const scope of JSON.parse(scopesKey) as Selection[]){
        if(abort.signal.aborted)return;
        try{
          const response=await api.agents(scope.environment_id,{project_id:scope.project_id,worktree_id:scope.worktree_id!,request_id:crypto.randomUUID(),action:'list'},abort.signal);
          if(abort.signal.aborted)return;
          for(const instance of response.instances)for(const item of ledger.current.observe(scope,instance)){
            if(!state.current.preferences.notifications)continue;
            setAttention(old=>[item,...old].slice(0,24));
            if(state.current.preferences.system_notifications){
              const {environment_id,project_id,worktree_id,instance_id,session_id,sequence}=item;
              void command('notify_attention',{target:{environment_id,project_id,worktree_id:worktree_id!,instance_id,session_id,sequence}}).then(reason=>{if(reason)setError(`Windows не сообщает доступность уведомлений (${reason}). Отправка запрошена; доставка не подтверждена. Attention остаётся в приложении.`);}).catch(error=>setError(error.message??'Windows notifications недоступны или запрещены. Attention остаётся в приложении.'));
            }
          }
        }catch{/* Transport failure is not a provider attention event. */}
      }
      if(!abort.signal.aborted)timer=setTimeout(()=>void poll(),2000);
    };void poll();return()=>{abort.abort();clearTimeout(timer);};
  },[api,scopesKey]);
  useEffect(()=>{if(!desktopMode())return;const subscription=onAttentionOpen(target=>openRef.current({...target,kind:'waiting',id:JSON.stringify(target)}));return()=>{void subscription.then(off=>off());};},[]);
  useEffect(()=>{if(!desktopMode())return;const subscription=onAttentionUnavailable(setError);return()=>{void subscription.then(off=>off());};},[]);
  useEffect(()=>{
    if(!target)return;
    if(selection?.environment_id!==target.environment_id || selection.project_id!==target.project_id || selection.worktree_id!==target.worktree_id){setTarget(null);return;}
    if(!state.current.trees.some(t=>t.environment_id===target.environment_id&&t.project_id===target.project_id&&t.id===target.worktree_id)){
      setError('Цель уведомления недоступна: среда отключена или worktree удалён.');setTarget(null);return;
    }
    let cancelled=false;const abort=new AbortController();
    void api.agents(target.environment_id,{action:'list',request_id:crypto.randomUUID(),project_id:target.project_id,worktree_id:target.worktree_id!},abort.signal).then(response=>{
      if(cancelled)return;
      const instance=response.instances.find(i=>i.id===target.instance_id&&i.connection==='connected'&&!i.stale&&i.sessions.some(s=>s.id===target.session_id&&s.assigned));
      if(instance)window.dispatchEvent(new CustomEvent('agent-focus',{detail:{...target,installation_id:instance.installation_id}}));
      else setError('Сессия уведомления больше не доступна. Автоматического запуска нет.');
      setTarget(null);
    }).catch(()=>{if(!cancelled){setError('Сессия уведомления недоступна.');setTarget(null);}});
    return()=>{cancelled=true;abort.abort();};
  },[api,target,selection,scopesKey]);
  const focus=()=>{const element=[...document.querySelectorAll<HTMLElement>('.overview .xterm-helper-textarea, .overview textarea:not(:disabled), .overview button:not(:disabled)')].find(e=>!e.closest('[hidden]')&&e.getClientRects().length>0);element?.focus();};
  const terminalTabs=()=>[...document.querySelectorAll<HTMLButtonElement>('.overview [role="tab"]')].filter(t=>!t.closest('[hidden]'));
  const cycle=(direction:number)=>{const tabs=terminalTabs();const active=tabs.findIndex(t=>t.getAttribute('aria-selected')==='true');tabs[(active+direction+tabs.length)%tabs.length]?.click();};
  const commands:Command[]=[
    {id:'palette',label:'Палитра команд',enabled:true,run:()=>{setSearch('');setIndex(0);setDialog('palette');}},
    {id:'add-project',label:'Добавить проект',enabled:!!onAddProject,run:()=>onAddProject?.()},
    ...(['overview','changes'] as const).map(id=>({id,label:{overview:desktopMode()?'Рабочая область':'Overview',changes:'Changes'}[id],enabled:id==='overview'||!!current,run:()=>view(id)})),
    {id:'terminal',label:'Рабочая область: вкладки',enabled:!!current&&desktopMode(),run:()=>view('overview')},
    {id:'explorer',label:'Показать / скрыть Explorer',enabled:ready,run:()=>arrange({...layout,explorer_visible:!layout.explorer_visible})},
    {id:'focus',label:'Фокус рабочей области',enabled:!!current,run:focus},
    {id:'next',label:'Следующая terminal tab',enabled:!!current&&terminalTabs().length>1,run:()=>cycle(1)},
    {id:'previous',label:'Предыдущая terminal tab',enabled:!!current&&terminalTabs().length>1,run:()=>cycle(-1)},
    {id:'preferences',label:'Настройки приложения',enabled:ready,run:()=>setDialog('preferences')},
    {id:'memory',label:'Диагностика памяти',enabled:ready&&desktopMode(),run:()=>setDialog('memory')},
    {id:'shortcuts',label:'Все сочетания клавиш',enabled:true,run:()=>setDialog('shortcuts')},
    ...buttonCommands.map(([id,label])=>({id,label,enabled:!!commandButton(id),run:()=>commandButton(id)?.click()})),
    ...trees.map(t=>({id:`tree:${t.environment_id}:${t.project_id}:${t.id}`,label:`Worktree: ${projects.find(p=>p.id===t.project_id&&p.environment_id===t.environment_id)?.name} / ${t.path} (${t.environment_id===selection?.environment_id?'текущая среда':t.environment_id})`,enabled:true,run:()=>select({environment_id:t.environment_id,project_id:t.project_id,worktree_id:t.id})})),
    {id:'ide',label:'Открыть worktree в IDE',enabled:!!current&&!!preferences.ide[current.environment_id]&&ready,run:()=>void openIde(current!)},
  ].map(c=>({...c,shortcut:shortcut(c.id)}));
  const commandsRef=useRef(commands);commandsRef.current=commands;
  useEffect(()=>{
    const key=(event:KeyboardEvent)=>{
      if(event.defaultPrevented||event.repeat||document.querySelector('dialog[open]'))return;
      const id=binding(event),action=commandsRef.current.find(c=>c.id===id&&c.enabled);
      if(!action)return;
      // Text/editor input retains all bindings except the explicit application palette.
      if(id!=='palette'&&event.target instanceof Element&&event.target.closest('input,textarea,[contenteditable="true"],.monaco-editor'))return;
      event.preventDefault();event.stopPropagation();action.run();
    };window.addEventListener('keydown',key,true);return()=>window.removeEventListener('keydown',key,true);
  },[]);
  async function openIde(tree:Worktree){
    const id=crypto.randomUUID();
    try{await api.terminal(tree.environment_id,{project_id:tree.project_id,worktree_id:tree.id,action:'open_ide',request_id:id,executable:preferences.ide[tree.environment_id]});
      setError('');
    }catch{setError('IDE не запущена. Нужен доступный executable в выбранной среде; Windows требует .exe. Проверьте диагностику Terminal.');}
  }
  const filtered=commands.filter(c=>c.id!=='palette'&&c.label.toLowerCase().includes(search.toLowerCase()));
  const active=Math.min(index,Math.max(0,filtered.length-1));
  useEffect(()=>{if(dialog==='palette')document.getElementById(`command-${active}`)?.scrollIntoView?.({block:'nearest'});},[dialog,active,search]);
  const run=(c:Command)=>{if(!c.enabled)return;setDialog(null);setTimeout(c.run,0);};
  return <div className="daily-controls">
    {desktopMode()&&<button className="icon-button" aria-label="Диагностика памяти" title="Диагностика памяти" onClick={()=>setDialog('memory')}><Icon name="server"/></button>}
    {dialog==='memory'&&<AgentDialog title="Диагностика памяти" close={()=>setDialog(null)}><MemoryDiagnostics/></AgentDialog>}
    <button className="command-launcher" aria-label="Команды" title={`Палитра команд · ${shortcut('palette')}`} onClick={()=>commands[0].run()}><Icon name="search"/><span>Найти или выполнить команду</span><kbd>{shortcut('palette')}</kbd></button>
    <button className="icon-button" aria-label="Настройки" title={`Настройки · ${shortcut('preferences')}`} onClick={()=>setDialog('preferences')}><Icon name="settings"/></button>
    <ActionMenu label="Attention" icon="bell" badge={attention.length}>{attention.length?attention.map(item=><button key={item.id} onClick={()=>openAttention(item)}><Icon name="bell"/><span>{attentionLabel[item.kind]}<small>{item.environment_id.slice(0,8)} / {item.worktree_id?.slice(0,8)} / {item.session_id.slice(0,12)}</small></span></button>):<p className="menu-empty">Новых подтверждённых событий нет.</p>}<button disabled={!attention.length} onClick={()=>setAttention([])}>Очистить список</button></ActionMenu>
    {failedDrafts.length > 0 && <div className="daily-message" role="alert">Не сохранены файлы Changes: {failedDrafts.map(d => <span key={d.key} title={d.error}><button onClick={() => { select({ environment_id: d.environment, project_id: d.request.project_id, worktree_id: d.request.worktree_id }); view('changes'); requestDraft(d); }}>{d.request.path}</button>{d.status === 'error' && <button onClick={() => void saveDraft(d).catch(() => {})}>Повторить</button>}</span>)}</div>}
    {error&&<div className="daily-message" role="status">{error}<button className="icon-button" aria-label="Скрыть сообщение" onClick={()=>setError('')}><Icon name="close" size={14}/></button></div>}
    {dialog==='palette'&&<AgentDialog title="Палитра команд" close={()=>setDialog(null)}>
      <input autoFocus aria-label="Поиск команды" role="combobox" aria-expanded="true" aria-controls="command-results" aria-activedescendant={filtered.length?`command-${active}`:undefined} value={search} onChange={e=>{setSearch(e.target.value);setIndex(0);}} onKeyDown={e=>{
        if(e.key==='ArrowDown'||e.key==='ArrowUp'){e.preventDefault();setIndex((active+(e.key==='ArrowDown'?1:-1)+filtered.length)%Math.max(1,filtered.length));}
        if(e.key==='Enter'&&filtered[active]){e.preventDefault();run(filtered[active]);}
      }}/><div id="command-results" className="command-results" role="listbox">{filtered.map((c,i)=><button id={`command-${i}`} role="option" aria-selected={active===i} aria-disabled={!c.enabled} key={c.id} onMouseEnter={()=>setIndex(i)} onClick={()=>run(c)}>{c.label}<kbd>{c.shortcut??(!c.enabled?'Недоступно':'')}</kbd></button>)}{!filtered.length&&<p>Команды не найдены.</p>}</div>
    </AgentDialog>}
    {dialog==='preferences'&&<AgentDialog title="Настройки приложения" close={()=>setDialog(null)}>
      <label>Тема<select aria-label="Тема" disabled={!ready} value={preferences.light_theme?'light':'dark'} onChange={e=>save({light_theme:e.target.value==='light'})}><option value="dark">Branchline dark (default)</option><option value="light">Branchline light</option></select></label>
      <label><input type="checkbox" disabled={!ready} checked={preferences.notifications} onChange={e=>save({notifications:e.target.checked})}/> Attention по подтверждённым событиям</label>
      <label><input type="checkbox" disabled={!ready||!preferences.notifications} checked={preferences.system_notifications} onChange={e=>save({system_notifications:e.target.checked})}/> Windows notifications (доступность зависит от настроек Windows)</label>
      <label><input type="checkbox" disabled={!ready} checked={preferences.close_without_confirmation} onChange={e=>save({close_without_confirmation:e.target.checked})}/> Закрывать без подтверждения</label>
      <p className="muted">Закрытие завершает app-owned терминалы, harness, IDE launcher и workers, включая несохранённую работу в запущенных ими процессах. Внешние процессы не присваиваются приложению. Restore не запускает процессы. Фонового режима нет.</p>
      <h3>IDE по среде</h3><p className="muted">VS Code compatible executable: Windows .exe или executable в явно выбранной WSL. Rust передаёт --new-window и проверенный worktree отдельными аргументами. Запущенные descendants принадлежат приложению; существующее окно IDE не присваивается ему. Не используйте launcher, который требует shell string.</p>
      {[...new Map(projects.map(p=>[p.environment_id,p])).values()].map(p=><label key={p.environment_id}>{environments.find(e=>e.id===p.environment_id)?.distro??'Windows'}<small>{p.environment_id}</small><input aria-label={`IDE ${p.environment_id}`} disabled={!ready} placeholder="Полный environment-local executable path" value={preferences.ide[p.environment_id]??''} onChange={e=>save({ide:{...preferences.ide,[p.environment_id]:e.target.value}})}/></label>)}
      <button onClick={()=>{setDialog(null);if(current)void openIde(current);}} disabled={!current||!preferences.ide[current.environment_id]||!ready}>Открыть выбранный worktree в IDE</button>
      <button onClick={()=>setDialog('shortcuts')}>Все сочетания клавиш</button>
    </AgentDialog>}
    {dialog==='shortcuts'&&<AgentDialog title="Сочетания клавиш" close={()=>setDialog(null)}><p className="muted">В input, xterm и Monaco доступна палитра Ctrl+Alt+P. Остальные сочетания приложения работают вне ввода и modal; стандартные сочетания редактора и терминала сохраняются. AltGr не перехватывается.</p>{commands.filter(c=>c.id in bindings).map(c=><p key={c.id}>{c.label} <kbd>{c.shortcut}</kbd></p>)}</AgentDialog>}
    {dialog==='close'&&<AgentDialog title="Закрыть приложение?" close={()=>setDialog(null)}><p>Все app-owned терминалы, harness, IDE launcher и workers будут завершены. Несохранённая работа в этих процессах может быть потеряна. Внешние процессы сохранятся. При следующем запуске восстановится layout, процессы потребуют явного запуска.</p><div className="dialog-actions"><button onClick={()=>setDialog(null)}>Отмена</button><button onClick={() => closeNow()}>Сохранить файлы и закрыть</button>{failedDrafts.length > 0 && <button onClick={() => closeNow(true)}>Отказаться от несохранённых изменений Changes и закрыть</button>}</div></AgentDialog>}
  </div>;
}
