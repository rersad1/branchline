import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { ApiClient, Worktree } from '../../api';
import type { AgentAction, AgentInstance, Installation, TerminalLayout } from '../../desktop';
import { TerminalWorkspace } from '../terminal/TerminalWorkspace';
import { AgentDialog } from './AgentDialog';
import { Icon } from '../../components/ui/icon';

function harnessName(installation: Installation) {
  return installation.name === 'opencode' ? 'OpenCode' : installation.name;
}
function candidate(installation?: Installation) {
  return !!installation?.npm && installation.capabilities.includes('D7 candidate validation');
}
function activityLabel(activity: string) {
  return ({ Working: 'Работает', Waiting: 'Ожидает ответа', Error: 'Ошибка', Idle: 'Готов' } as Record<string,string>)[activity] ?? '';
}

export function HarnessPanel({ api, tree, connected, layout, saveLayout, diagnostics }: {
  api: ApiClient; tree: Worktree; connected: boolean; layout?: TerminalLayout;
  saveLayout?: (value: TerminalLayout) => void; diagnostics?: ReactNode;
}) {
  const [installations, setInstallations] = useState<Installation[]>([]), [selected, setSelected] = useState('');
  const [instances, setInstances] = useState<AgentInstance[]>([]), [active, setActive] = useState(''), [session, setSession] = useState('');
  const [mode, setMode] = useState<'terminal'|'chat'>('terminal'), [dialog, setDialog] = useState<'settings'|'diagnostics'|null>(null);
  const [manual, setManual] = useState(''), [port, setPort] = useState(''), [prompt, setPrompt] = useState('');
  const [node, setNode] = useState('');
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [stale, setStale] = useState(true), [discovering, setDiscovering] = useState(false);
  const fallback: TerminalLayout = { environment_id: tree.environment_id, project_id: tree.project_id, worktree_id: tree.id, tabs: [], active_id: null, split_id: null, vertical: false };
  const [localLayout, setLocalLayout] = useState(fallback);
  const terminalLayout = layout ?? localLayout;
  const layoutRef = useRef(terminalLayout), saveRef = useRef(saveLayout);
  layoutRef.current = terminalLayout; saveRef.current = saveLayout;
  const live = useRef<AbortController|null>(null), revision = useRef(0), busyRef = useRef(false);
  const discovery = useRef(0), chatLog = useRef<HTMLDivElement>(null), followOutput = useRef(true);
  const scope = { project_id: tree.project_id, worktree_id: tree.id };

  useEffect(() => {
    const controller = new AbortController(); live.current = controller; let running = true;
    if (connected) void discover(controller.signal);
    void (async () => { while (running) {
      try {
        if (connected && !busyRef.current) {
          const version = revision.current;
          const result = await api.agents(tree.environment_id, { ...scope, request_id: crypto.randomUUID(), action: 'list' }, controller.signal);
          if (!running) return;
          if (version === revision.current) { setInstances(result.instances); setStale(false); }
        }
      } catch (error) { if (!controller.signal.aborted) { setStale(true); setError((error as Error).message); } }
      await new Promise(resolve => setTimeout(resolve, 800));
    } })();
    return () => { running = false; controller.abort(); };
  }, [api, tree.environment_id, tree.project_id, tree.id, connected]);

  useEffect(() => {
    const focus = (event: Event) => {
      const target = (event as CustomEvent).detail;
      if (target.environment_id !== tree.environment_id || target.project_id !== tree.project_id || target.worktree_id !== tree.id) return;
      setSelected(target.installation_id); setActive(target.instance_id); setSession(target.session_id); setMode('chat');
    };
    window.addEventListener('agent-focus', focus);
    return () => window.removeEventListener('agent-focus', focus);
  }, [tree.environment_id, tree.project_id, tree.id]);

  async function discover(signal = live.current?.signal, path?: string) {
    const version = ++discovery.current; setDiscovering(true);
    try {
      const result = await api.harnesses(tree.environment_id, { request_id: crypto.randomUUID(), ...(path ? { action: 'add' as const, executable: path, ...(node ? {node} : {}) } : { action: 'discover' as const }) }, signal);
      if (signal?.aborted || version !== discovery.current) return;
      setInstallations(result.installations);
      setSelected(old => result.installations.some(i => i.id === old) ? old : result.installations.find(i => i.name === 'opencode')?.id ?? result.installations[0]?.id ?? '');
      setError('');
    } catch (error) { if (!signal?.aborted && version === discovery.current) setError((error as Error).message); }
    finally { if (!signal?.aborted && version === discovery.current) setDiscovering(false); }
  }
  async function control(action: AgentAction) {
    if (busyRef.current) return false;
    busyRef.current = true; revision.current++; setBusy(true); setError('');
    const signal = live.current?.signal;
    try {
      const result = await api.agents(tree.environment_id, { ...scope, request_id: crypto.randomUUID(), ...action }, signal);
      if (!signal?.aborted) { setInstances(result.instances); setStale(false); return true; }
      return false;
    } catch (error) { if (!signal?.aborted) { setError((error as Error).message); setStale(true); } return false; }
    finally { busyRef.current = false; if (!signal?.aborted) setBusy(false); }
  }
  function persistTerminal(value: TerminalLayout) {
    layoutRef.current = value;
    setLocalLayout(value); saveRef.current?.(value);
  }
  async function launch() {
    const installation = installations.find(i => i.id === selected);
    if (!installation || busyRef.current || !connected) return;
    if (mode === 'chat') {
      if (installation.protocol !== 'acp' && !candidate(installation)) return;
      const id = crypto.randomUUID(); setActive(id); setSession('');
      window.dispatchEvent(new CustomEvent('agent-launched',{detail:{environment_id:tree.environment_id,project_id:tree.project_id,worktree_id:tree.id,instance_id:id}}));
      await control({ action: 'launch', installation_id: installation.id, instance_id: id, ...(candidate(installation) ? {candidate:true} : {}) }); return;
    }
  }

  const activeTab=terminalLayout.tabs.find(t=>t.id===terminalLayout.active_id);
  useEffect(()=>{setMode('terminal');const launch=layoutRef.current.tabs.find(t=>t.id===layoutRef.current.active_id)?.launch;if(launch?.kind==='harness')setSelected(launch.installation_id);},[terminalLayout.active_id]);

  const installation = installations.find(i => i.id === selected);
  const chats = instances.filter(i => (i.owned && i.installation_id === selected) || i.id === active);
  const instance = chats.find(i => i.id === active) ?? chats[0];
  const current = instance?.sessions.find(s => s.id === session && s.assigned) ?? instance?.sessions.find(s => s.assigned);
  const ready = connected && !stale && instance?.connection === 'connected' && !instance.stale && instance.owned && instance.protocol_version === 1;
  const sessions = chats.flatMap(i => i.sessions.filter(s => s.assigned).map((s, index) => ({ instance: i, session: s, label: `Сессия ${index + 1}${chats.length > 1 ? ` · ${i.id.slice(0,4)}` : ''}` })));
  const waiting = instances.find(i => i.permissions.length > 0);
  const messages: { kind: string; text: string; key: number }[] = [];
  for (const event of instance?.events.filter(e => e.session_id === current?.id && ['agent_message_chunk','user_message_chunk','tool_call','tool_call_update'].includes(e.kind)) ?? []) {
    const kind = event.kind === 'user_message_chunk' ? 'Вы' : event.kind.startsWith('tool_') ? 'Инструмент' : 'Агент';
    const last = messages.at(-1);
    if (last && last.kind === kind && kind !== 'Инструмент') last.text += event.text;
    else messages.push({ kind, text: event.text, key: event.sequence });
  }
  useEffect(() => { if (followOutput.current && chatLog.current) chatLog.current.scrollTop = chatLog.current.scrollHeight; }, [instance?.cursor, current?.id, mode]);
  const hasWorkspace = mode === 'terminal' ? terminalLayout.tabs.length > 0 : !!instance;
  const connectionLost = mode === 'chat' && instance && (stale || instance.stale || ['disconnected','stopped'].includes(instance.connection));
  const protocolError = instance?.events.some(e => e.kind === 'error');
  const canLaunch = connected && !!installation && !busy && (mode !== 'chat' || installation.protocol === 'acp' || candidate(installation)) && (mode !== 'terminal' || terminalLayout.tabs.length < 8);
  const newSession = () => mode === 'chat' && ready && instance ? control({ action: 'new', instance_id: instance.id }) : launch();

  return <section className="harness-panel agent-workspace" aria-label="Harness workspace">
    <div className="agent-toolbar">
      {mode==='chat'&&<select aria-label="Installed harness" value={selected} disabled={busy} onChange={event => { setSelected(event.target.value); setActive(''); setSession(''); if (installations.find(i => i.id === event.target.value)?.protocol !== 'acp') setMode('terminal'); }}>
        <option value="">{discovering ? 'Поиск harness…' : 'Выберите harness'}</option>
        {installations.map(i => <option key={i.id} value={i.id} title={i.executable}>{harnessName(i)}{i.version ? ` · ${i.version}` : ''}{installations.filter(other => other.name === i.name).length > 1 ? ` · ${i.executable.replaceAll('\\','/').split('/').at(-2)}` : ''}</option>)}
      </select>}
      <div className="agent-mode" role="group" aria-label="Режим агента">
        <button aria-pressed={mode === 'terminal'} onClick={() => setMode('terminal')}><Icon name="terminal" size={14}/>Терминал</button>
        <button aria-pressed={mode === 'chat'} disabled={activeTab?.launch?.kind==='shell'||(mode==='terminal'&&activeTab?.launch?.kind==='harness'&&activeTab.launch.installation_id!==installation?.id)||(installation?.protocol !== 'acp' && !candidate(installation))} onClick={() => setMode('chat')}><Icon name="chat" size={14}/>Чат</button>
      </div>
      {mode === 'chat' && sessions.length > 0 && <select aria-label="Agent session" value={`${instance?.id}/${current?.id}`} onChange={event => { const value = sessions.find(item => `${item.instance.id}/${item.session.id}` === event.target.value); if (value) { setActive(value.instance.id); setSession(value.session.id); followOutput.current = true; } }}>
        {sessions.map(item => <option key={`${item.instance.id}/${item.session.id}`} value={`${item.instance.id}/${item.session.id}`}>{item.label}{ready && activityLabel(item.session.activity) ? ` · ${activityLabel(item.session.activity)}` : ''}</option>)}
      </select>}
      {mode==='chat'&&hasWorkspace && <button className="agent-new" disabled={!canLaunch || instance?.connection === 'connecting'} onClick={() => void newSession()}><Icon name="plus" size={14}/>Новая сессия</button>}
      <details className="agent-menu" onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.currentTarget.querySelector('summary')?.focus(); } }}>
        <summary aria-label="Действия агента" title="Действия агента"><Icon name="more"/></summary>
        <div className="agent-menu-items">
          <button data-command="harness" onClick={event => { event.currentTarget.closest('details')!.open = false; setDialog('settings'); }}><Icon name="settings"/>Настроить harness</button>
          <button data-command="diagnostics" onClick={event => { event.currentTarget.closest('details')!.open = false; setDialog('diagnostics'); }}><Icon name="worktree"/>Диагностика и worktree</button>
          {mode === 'chat' && instance?.capabilities.list && <button data-command="history" disabled={!ready || busy} onClick={() => void control({ action: 'history', instance_id: instance.id })}>История сессий</button>}
          {mode === 'chat' && instance?.capabilities.load && current && <button data-command="load" disabled={!ready || busy} onClick={() => void control({ action: 'load', instance_id: instance.id, session_id: current.id })}>Загрузить сессию</button>}
          {mode === 'chat' && current && current.modes.length > 0 && <label>Режим сессии<select aria-label="Agent mode" disabled={!ready || busy} value={current.current_mode ?? ''} onChange={event => void control({ action: 'mode', instance_id: instance!.id, session_id: current.id, mode_id: event.target.value })}>{current.modes.map(m => <option key={m.id} value={m.id}>{m.name}</option>)}</select></label>}
          {mode === 'chat' && current?.config_options?.map(option => <label key={option.id}>{option.name}<select aria-label={`Agent configuration ${option.name}`} disabled={!ready || busy || ['Working','Waiting'].includes(current.activity)} value={option.current_value} onChange={event => void control({ action: 'config', instance_id: instance!.id, session_id: current.id, config_id: option.id, value: event.target.value })}>
            {!option.options.some(value => value.id === option.current_value) && <option value={option.current_value} disabled>{option.current_value}</option>}
            {option.options.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}
          </select></label>)}
          {mode === 'chat' && current?.commands?.map(command => <button key={command.name} title={command.description} disabled={!ready || busy} onClick={event => { setPrompt(draft => `/${command.name} ${draft}`); event.currentTarget.closest('details')!.open = false; }}>{`/${command.name}`}</button>)}
          {mode === 'chat' && instance && !['stopped','disconnected'].includes(instance.connection) && <button disabled={!connected || busy} onClick={() => void control({ action: 'stop', instance_id: instance.id })}>Остановить агента</button>}
        </div>
      </details>
    </div>

    {!connected && <p className="agent-notice" role="alert">Среда недоступна. Проверьте подключение в Explorer.</p>}
    {error && <div className="agent-notice" role="alert"><span>Действие не завершено.</span><button onClick={() => setDialog('diagnostics')}>Подробнее</button><button className="icon-button" aria-label="Скрыть сообщение" onClick={() => setError('')}><Icon name="close" size={14}/></button></div>}
    {waiting && (mode !== 'chat' || waiting.id !== instance?.id) && <div className="agent-notice attention"><span>Агент ожидает вашего ответа</span><button onClick={() => { setSelected(waiting.installation_id); setActive(waiting.id); setSession(waiting.permissions[0].session_id); setMode('chat'); }}>Открыть сессию</button></div>}

    <div className="agent-native" hidden={mode !== 'terminal'}><TerminalWorkspace api={api} tree={tree} connected={connected} layout={terminalLayout} save={persistTerminal} harness={installation} installations={installations} onSettings={()=>setDialog('settings')} onActivate={target=>{setMode('terminal');if(target)setSelected(target.id);}} compact launching={busy}/></div>
    {mode === 'terminal' ? null : instance ? <div className="agent-chat">
      {connectionLost && <div className="agent-notice" role="alert"><span>{instance.connection === 'stopped' ? 'Агент остановлен.' : 'Связь с агентом потеряна.'}</span><button disabled={!canLaunch} onClick={() => void launch()}>Запустить заново</button></div>}
      {!connectionLost && protocolError && <div className="agent-notice" role="alert"><span>Агент отклонил запрос.</span><button onClick={() => setDialog('diagnostics')}>Подробнее</button></div>}
      <div ref={chatLog} className="agent-conversation" role="log" aria-label="Сообщения агента" onScroll={event => { const element = event.currentTarget; followOutput.current = element.scrollHeight - element.scrollTop - element.clientHeight < 60; }}>
        {instance.events.length > 0 && instance.events[0].sequence > 1 && <p className="muted">Показана последняя часть переписки.</p>}
        {messages.map(message => <div className={`agent-message ${message.kind === 'Инструмент' ? 'tool' : ''}`} key={message.key}><span>{message.kind}</span><p>{message.text}</p></div>)}
        {!current && <div className="agent-empty"><h2>{instance.connection === 'connecting' ? 'Подключение…' : 'Начните новую сессию'}</h2>{ready && <button disabled={busy} onClick={() => void newSession()}>Новая сессия</button>}</div>}
        {current && !messages.length && <div className="agent-empty"><h2>Чем займёмся?</h2><p>Напишите запрос агенту ниже.</p></div>}
      </div>
      {instance.permissions.map(permission => <div key={permission.id} className="harness-permission"><strong>Требуется разрешение: {permission.title}</strong>{permission.session_id !== current?.id && <span>Другая сессия</span>}{permission.options.map(option => <button key={option.id} disabled={!ready || busy} onClick={() => void control({ action: 'permission', instance_id: instance.id, permission_id: permission.id, option_id: option.id })}>{option.name}</button>)}<button disabled={!ready || busy} onClick={() => void control({ action: 'permission', instance_id: instance.id, permission_id: permission.id, option_id: null })}>Отказать</button></div>)}
      {current && <form className="agent-composer" onSubmit={event => {
        event.preventDefault();
        const submitted = prompt;
        if (!ready || busyRef.current || !submitted.trim() || new TextEncoder().encode(submitted).length > 8192) return;
        followOutput.current = true;
        void control({ action: 'prompt', instance_id: instance.id, session_id: current.id, text: submitted }).then(sent => {
          if (sent) setPrompt(draft => draft === submitted ? '' : draft);
        });
      }}>
        <textarea aria-label="Agent prompt" placeholder="Напишите запрос…" maxLength={8192} value={prompt} onChange={event => setPrompt(event.target.value)} disabled={!ready}/>
        <div><span className="muted">{ready ? activityLabel(current.activity) : ''}</span><button type="button" title="Остановить ответ" aria-label="Остановить ответ" className="icon-button" disabled={!ready || busy} onClick={() => void control({ action: 'cancel', instance_id: instance.id, session_id: current.id })}><Icon name="stop"/></button><button className="agent-primary" disabled={!ready || busy || !prompt.trim() || new TextEncoder().encode(prompt).length > 8192 || ['Working','Waiting'].includes(current.activity)}><Icon name="send" size={14}/>Отправить</button></div>
      </form>}
    </div> : <div className="agent-empty"><h2>{installation ? harnessName(installation) : 'Выберите harness для работы'}</h2>{mode === 'chat' && candidate(installation) && <p>Совместимость D7 ещё не подтверждена. Тест запускает выбранный harness в этом worktree; запросы могут расходовать лимит провайдера.</p>}{installation ? <button className="agent-primary" disabled={!canLaunch} onClick={() => void launch()}>{busy ? 'Запуск…' : mode === 'chat' && candidate(installation) ? 'Тест ACP D7' : 'Запустить'}</button> : <button onClick={() => setDialog('settings')}>Настроить harness</button>}</div>}

    {dialog === 'settings' && <AgentDialog title="Настроить harness" close={() => setDialog(null)}>
      <p className="muted">Установки в выбранной среде. Добавление не запускает агента.</p>
      <button disabled={!connected || discovering} onClick={() => void discover()}>{discovering ? 'Поиск…' : 'Обнаружить установки'}</button>
      <div className="agent-installations">{installations.map(i => <label key={i.id}><input type="radio" name="harness-installation" checked={selected === i.id} onChange={() => { setSelected(i.id); if (i.protocol !== 'acp') setMode('terminal'); }}/><div><strong>{harnessName(i)} · {i.version ?? 'Версия не проверена'}</strong><code>{i.executable}</code><p>{i.compatibility}</p><p className="muted">{i.capabilities.join(' · ')}</p></div></label>)}</div>
      <form onSubmit={event => { event.preventDefault(); void discover(live.current?.signal, manual); }}><label htmlFor="manual-harness">Executable или папка npm-пакета в выбранной среде</label><div className="path-controls"><input id="manual-harness" aria-label="Manual harness executable" placeholder="Полный путь" value={manual} onChange={event => setManual(event.target.value)}/><button disabled={!manual || !connected || discovering}>Добавить</button></div><label>Node для npm-пакета (пусто — поиск в PATH)<input aria-label="Harness Node executable" placeholder="Полный путь к node.exe или node" value={node} onChange={event => setNode(event.target.value)}/></label></form>
      {error && <p role="alert">{error}</p>}
      <div className="dialog-actions"><button onClick={() => setDialog(null)}>Готово</button></div>
    </AgentDialog>}
    {dialog === 'diagnostics' && <AgentDialog title="Диагностика и worktree" close={() => setDialog(null)}>
      {error && <p role="alert">{error}</p>}
      {instances.map(i => <details key={i.id}><summary>{installations.find(item => item.id === i.installation_id)?.name ?? 'OpenCode HTTP/SSE'} · {i.connection}</summary><dl><dt>Instance</dt><dd>{i.id}</dd><dt>Directory</dt><dd>{i.directory}</dd><dt>Protocol</dt><dd>{i.protocol_version ? `ACP ${i.protocol_version}` : 'HTTP/SSE'}</dd><dt>Capabilities</dt><dd>load {String(i.capabilities.load)} · list {String(i.capabilities.list)} · image {String(i.capabilities.image)} · audio {String(i.capabilities.audio)}</dd><dt>Error</dt><dd>{i.error ?? '—'}</dd></dl><p>Text input only. Additional CLI/TUI features are available in Terminal.</p>{i.events.filter(event => event.kind === 'error').map(event => <p key={event.sequence}>{event.text}</p>)}{!i.owned && i.connection === 'connected' && <button disabled={busy || !connected} onClick={() => void control({ action: 'stop', instance_id: i.id })}>Отключить наблюдение</button>}</details>)}
      <details><summary>Внешний OpenCode HTTP/SSE</summary><form onSubmit={event => { event.preventDefault(); void control({ action: 'attach', port: Number(port), instance_id: crypto.randomUUID() }); }}><label>Local port <input aria-label="External OpenCode port" type="number" min="1" max="65535" value={port} onChange={event => setPort(event.target.value)}/></label><button disabled={!connected || busy || !port}>Подключить наблюдение</button></form></details>
      {diagnostics}
    </AgentDialog>}
  </section>;
}


