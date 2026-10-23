import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { QueryClient, QueryClientProvider, useQuery, useQueries, useQueryClient } from '@tanstack/react-query';
import { ApiClient, ApiError, worktreeCacheKey, type Project } from './api';
import { desktopMode, onRuntimeState, type RuntimeState, type Selection, type Layout } from './desktop';
import { Button } from './components/ui/button';
import { Dashboard, treeName, branchLabel, commitLabel } from './features/dashboard/Dashboard';
import { AddProject } from './features/dashboard/AddProject';
import { DailyUse } from './features/daily/DailyUse';
import { Icon } from './components/ui/icon';

export function Problem({ error, retry }: { error: Error; retry: () => void }) {
  return <div role="alert" className="notice"><strong>{error instanceof ApiError && error.kind === 'disconnected' ? 'Disconnected' : 'Error'}</strong><p>{error.message}</p><Button variant="outline" onClick={retry}>Повторить</Button></div>;
}
export function Shell({ api }: { api: ApiClient }) {
  const [adding, setAdding] = useState(false);
  const [actionError, setActionError] = useState('');
  const [busy, setBusy] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [view, setView] = useState<'overview' | 'changes' | 'terminal'>('overview');
  const [layout, setLayout] = useState<Layout>({ explorer_visible: true, explorer_width: 280, collapsed: [] });
  const [restored, setRestored] = useState(!desktopMode());
  const saves = useRef<Promise<unknown>>(Promise.resolve());
  const saveFailure = useRef<unknown>(null);
  const client = useQueryClient();
  useEffect(() => {
    const saved = () => { void client.invalidateQueries({ queryKey: ['worktrees'] }); };
    window.addEventListener('changes-saved', saved);
    return () => window.removeEventListener('changes-saved', saved);
  }, [client]);
  const local = useQuery({ queryKey: ['environment'], queryFn: ({ signal }) => api.environment(signal), refetchInterval: 5000 });
  const workspace = useQuery({ queryKey: ['workspace'], enabled: desktopMode(), queryFn: () => api.loadWorkspace() });
  const states = useQuery({ queryKey: ['runtime-states'], enabled: desktopMode(), queryFn: () => api.runtimeStates(), refetchInterval: 2000 });
  useEffect(() => {
    if (workspace.data && !restored) { setSelection(workspace.data.selection); setLayout(workspace.data.layout); setRestored(true); }
  }, [workspace.data, restored]);
  useEffect(() => {
    if (!desktopMode()) return;
    const subscription = onRuntimeState(state => {
      client.setQueryData<RuntimeState[]>(['runtime-states'], old => [...(old ?? []).filter(s => s.environment.id !== state.environment.id), state]);
      void client.invalidateQueries({ queryKey: ['runtime-states'] });
      void client.invalidateQueries({ queryKey: ['projects', state.environment.id] });
      void client.invalidateQueries({ queryKey: ['worktrees', state.environment.id] });
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [client]);
  function persist(nextSelection: Selection | null, nextLayout: Layout) {
    if (!desktopMode() || !restored || workspace.isError) return;
    saves.current = saves.current.catch(() => {}).then(async () => { await api.saveWorkspace(nextSelection, nextLayout); saveFailure.current = null; }).catch(e => { saveFailure.current = e; setActionError(`Сохранение UI: ${(e as Error).message}`); });
  }
  function select(value: Selection | null) { setSelection(value); persist(value, layout); }
  function arrange(value: Layout) { setLayout(value); persist(selection, value); }
  const environments: RuntimeState[] = [
    ...(local.data ? [{ environment: local.data, state: local.isError ? 'error' as const : 'connected' as const, error: local.error ? { code: 'local_unavailable', message: local.error.message } : null }] : []),
    ...(states.data?.map(s => states.isError ? { ...s, state: 'disconnected' as const, error: { code: 'disconnected', message: states.error.message } } : s) ?? workspace.data?.environments.map(r => ({ environment: r.environment, state: 'disconnected' as const, error: { code: 'disconnected', message: 'Восстановление соединения…' } })) ?? []),
  ];
  const projectQueries = useQueries({ queries: environments.map(state => ({ queryKey: ['projects', state.environment.id], enabled: state.state === 'connected', queryFn: ({ signal }: { signal: AbortSignal }) => api.projects(signal, state.environment.id), refetchInterval: 5000 })) });
  const projects = projectQueries.flatMap(q => q.data ?? []);
  const views = environments.map((state, i) => ({ ...state, projects_error: projectQueries[i].error?.message, loading: state.state === 'connected' && projectQueries[i].isPending,
    // A background fetch does not invalidate an already observed empty registry.
    projects_loaded: state.state === 'connected' && projectQueries[i].isSuccess,
    authoritative: state.state === 'connected' && projectQueries[i].isSuccess && !projectQueries[i].isFetching }));
  // Prune collapse references only after their environment's registry is authoritative.
  useEffect(() => {
    if (!restored) return;
    const valid = layout.collapsed.filter(id => {
      const parts = id.split(':');
      if (!['env', 'repo'].includes(parts[0])) return false;
      const view = views.find(v => v.environment.id === parts[1]);
      if (!view) return local.isSuccess && states.isSuccess && workspace.isSuccess ? false : true;
      return parts[0] === 'env' || !view.authoritative || projects.some(p => p.environment_id === parts[1] && p.id === parts[2]);
    });
    if (valid.length !== layout.collapsed.length) arrange({ ...layout, collapsed: valid });
  }, [projects.map(p => `${p.environment_id}:${p.id}`).join(','), states.data, restored]);
  async function run(operation: () => Promise<unknown>) {
    setBusy(true); setActionError('');
    try { await operation(); await states.refetch(); await client.invalidateQueries({ queryKey: ['workspace'] }); }
    catch (e) { setActionError((e as Error).message); await states.refetch(); }
    finally { setBusy(false); }
  }
  async function remove(project: Project) {
    await saves.current;
    try { await api.removeProject(project.environment_id, project.id); }
    finally { await client.invalidateQueries({ queryKey: ['projects', project.environment_id] }); }
    client.removeQueries({ queryKey: ['worktrees', project.environment_id, project.id] });
    await client.invalidateQueries({ queryKey: ['workspace'] });
  }
  const selectedEnv = environments.find(e => e.environment.id === selection?.environment_id);
  const selectedProject = projects.find(p => p.environment_id === selection?.environment_id && p.id === selection.project_id);
  const selectedSnapshot = useQuery({ queryKey: worktreeCacheKey(selection?.environment_id ?? '', selection?.project_id ?? ''), enabled: !!selectedProject && selectedEnv?.state === 'connected', queryFn: ({ signal }) => api.worktrees(selection!.project_id, signal, selection!.environment_id) });
  const selectedTree = selectedSnapshot.data?.worktrees.find(t => t.id === selection?.worktree_id);
  useEffect(() => {
    if (selection && local.isSuccess && states.isSuccess && workspace.isSuccess && !environments.some(e => e.environment.id === selection.environment_id)) select(null);
  }, [selection, local.isSuccess, states.data, workspace.data]);
  return <>
    <header className="workspace-bar">
      <div className="workspace-brand"><button className="icon-button explorer-toggle" aria-label={layout.explorer_visible ? 'Скрыть Explorer' : 'Показать Explorer'} title="Explorer · Ctrl+Alt+E" aria-pressed={layout.explorer_visible} onClick={() => arrange({ ...layout, explorer_visible: !layout.explorer_visible })}><Icon name="panels"/></button><h1><Icon name="branch" size={19}/>Branchline</h1></div>
      {selection && <nav className="context-tabs header-tabs" aria-label="Worktree views">{([
        ['overview', desktopMode()?'Рабочая область':'Overview', 'terminal'], ['changes', 'Changes', 'changes'],
      ] as [typeof view, string, 'grid' | 'changes' | 'terminal'][]).map(([id, label, icon], index) => <button key={id} title={`Ctrl+Alt+${index + 1}`} aria-current={view === id ? 'page' : undefined} className={view === id ? 'active-tab' : ''} disabled={id !== 'overview' && !selection.worktree_id} onClick={() => setView(id)}><Icon name={icon}/>{label}</button>)}</nav>}
      <DailyUse api={api} layout={layout} arrange={arrange} selection={selection} select={select} view={setView} projects={projects} connected={environments.filter(e=>e.state==='connected').map(e=>e.environment.id)} ready={restored&&!workspace.isError} environments={environments.map(e=>e.environment)} onAddProject={desktopMode() && local.data && !busy ? () => setAdding(true) : undefined} beforeClose={async()=>{await saves.current;if(saveFailure.current)throw saveFailure.current;}}/>
    </header>
    {adding && local.data && <AddProject api={api} windows={local.data} close={() => setAdding(false)} added={env => { setAdding(false); void states.refetch(); void client.invalidateQueries({ queryKey: ['workspace'] }); void client.invalidateQueries({ queryKey: ['projects', env.id] }); }}/ >}
    {actionError && <p role="alert" className="shell-error">{actionError}</p>}
    {local.error && <Problem error={local.error} retry={() => { void local.refetch(); }}/ >}
    {workspace.error && <Problem error={workspace.error} retry={() => { void workspace.refetch(); }}/ >}
    {states.error && <Problem error={states.error} retry={() => { void states.refetch(); }}/ >}
    {!restored && !workspace.error ? <p role="status">Loading — workspace…</p> : <Dashboard addProjectControl={desktopMode() ? <button className="add-project-action" data-command="add-project" disabled={!local.data || busy} onClick={() => setAdding(true)}><Icon name="plus"/>Добавить проект</button> : undefined} onWorktreeRemoved={(nextSelection,nextLayout)=>{setSelection(nextSelection);setLayout(nextLayout);persist(nextSelection,nextLayout);}} api={api} projects={projects} view={view} onView={setView} environments={views} selection={selection} layout={layout} onSelection={select} onLayout={arrange} remove={remove} retry={env => { void run(() => env.kind === 'wsl' ? api.connectWsl(env.distro!, env.id) : local.refetch()); }} relink={env => { if (window.confirm(`Перепривязать ${env.distro} к текущей установке? Старый выбор будет очищен. Registry текущей distro будет прочитан заново.`)) void run(async () => { await saves.current; await api.relinkWsl(env.id); client.removeQueries({ queryKey: ['projects', env.id] }); client.removeQueries({ queryKey: ['worktrees', env.id] }); if (selection?.environment_id === env.id) select(null); }); }}/ >}
    <div className="status-bar"><span title={selectedTree ? `${selectedTree.path} · ${branchLabel(selectedTree)} · ${commitLabel(selectedTree)}` : selectedProject?.root_path}>{selectedProject?.name ?? 'Workspace'} {selectedTree ? ` / ${treeName(selectedTree)} · ${branchLabel(selectedTree)} · ${commitLabel(selectedTree)}` : selection?.worktree_id ? ' / Worktree недоступен' : ''}</span><span>{selectedEnv ? `${selectedEnv.environment.distro ?? 'Windows'} · ${selectedEnv.state}` : 'Выберите Worktree'}</span><span>Read-only Git discovery</span></div>
  </>;
}
export default function App() {
  const [token, setToken] = useState('');
  const [session, setSession] = useState<string | null>(() => desktopMode() ? '' : null);
  const queryClient = useMemo(() => new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: true } } }), [session]);
  const api = useMemo(() => new ApiClient(session ?? ''), [session]);
  function connect(event: FormEvent) { event.preventDefault(); setSession(token); setToken(''); }
  return <main>
    {session === null ? <section className="login"><p className="eyebrow">LOCAL WORKSPACE</p><h1>Agent Workspace</h1><p>Подключение к Rust backend в Ubuntu WSL2</p><form onSubmit={connect}><label htmlFor="token">Локальный токен backend</label><input id="token" type="password" autoComplete="off" value={token} onChange={event => setToken(event.target.value)} minLength={32} maxLength={256} pattern="[A-Za-z0-9_-]{32,256}" required/><p className="muted">Хранится только в памяти этой вкладки. После перезагрузки введите снова.</p><Button type="submit">Подключиться</Button></form></section> : <QueryClientProvider client={queryClient}><Shell api={api}/><footer>{!desktopMode() && <Button variant="outline" onClick={() => { queryClient.clear(); setSession(null); }}>Отключиться и забыть токен</Button>}</footer></QueryClientProvider>}
  </main>;
}
