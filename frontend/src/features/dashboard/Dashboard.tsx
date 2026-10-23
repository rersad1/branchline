import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { worktreeCacheKey, type ApiClient, type Project, type Snapshot, type Worktree, type ExecutionEnvironment } from '../../api';
import { Button } from '../../components/ui/button';
import { desktopMode, type Selection, type Layout } from '../../desktop';
import { gitView, timestamp } from './status';
import { Changes } from '../changes/Changes';
import { ExplorerResize } from './ExplorerResize';
import { RemoveWorktree } from './RemoveWorktree';
import { CreateWorktree } from './CreateWorktree';
import { HarnessPanel } from './HarnessPanel';
import { ProviderCheck } from './ProviderCheck';
import { WorkspaceView } from './WorkspaceView';
import { Icon } from '../../components/ui/icon';
import { ActionMenu } from '../../components/ui/action-menu';
import { ExplorerItem } from './ExplorerItem';

function GitSummary({ tree, snapshot, failed, now }: { tree: Worktree; snapshot: Snapshot; failed: boolean; now: number }) {
  const view = gitView(tree, snapshot, failed, now);
  return <div className={`git-summary ${view.current ? 'current' : 'stale'} ${view.label === 'Conflicts' ? 'conflicts' : ''}`}>
    <span className="badge">{view.label}</span>
    {!view.current && <p className="status-reason">{view.reason}</p>}
    <p className="counts" aria-label={view.current ? 'Подтверждённые Git counts' : 'Последние известные Git counts'}>
      {!view.current && <span>Последние известные: </span>}
      <span>Staged: {tree.git_status.staged ?? '—'}</span>
      <span>Unstaged: {tree.git_status.unstaged ?? '—'}</span>
      <span>Untracked: {tree.git_status.untracked ?? '—'}</span>
      <span>Conflicts: {tree.git_status.conflicts ?? '—'}</span>
    </p>
    <p className="muted">Git проверен: {timestamp(tree.git_status.last_successful_refresh_at)}</p>
  </div>;
}
function TreeFlags({ tree }: { tree: Worktree }) {
  return <div className="flags">
    {tree.is_main && <span className="badge">Primary</span>}
    {tree.is_bare && <span className="badge">Bare</span>}
    {tree.is_detached && <span className="badge">Detached HEAD</span>}
    {!tree.is_available && <span className="badge unavailable">Недоступно / missing</span>}
    {tree.is_locked && <span className="badge">Locked{tree.lock_reason ? `: ${tree.lock_reason}` : ''}</span>}
    {tree.is_prunable && <span className="badge">Prunable{tree.prune_reason ? `: ${tree.prune_reason}` : ''}</span>}
  </div>;
}
export const treeName = (tree: Pick<Worktree, 'path'>) => tree.path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || tree.path;
export const commitLabel = (tree: Pick<Worktree, 'head'>) => tree.head && !/^0+$/.test(tree.head) ? tree.head.slice(0, 10) : 'Нет commit';
export const branchLabel = (tree: Worktree) => tree.is_detached ? `Detached · ${tree.head?.slice(0, 10) ?? 'Нет commit'}` : tree.branch?.replace(/^refs\/heads\//, '') ?? (tree.is_bare ? 'Bare repository' : 'Нет commit');
const key = (tree: Worktree) => JSON.stringify([tree.environment_id, tree.project_id, tree.id]);
export interface EnvironmentView { environment: ExecutionEnvironment; state: string; error: { code: string; message: string } | null; projects_error?: string; loading?: boolean; projects_loaded?: boolean; authoritative?: boolean }
const defaultLayout: Layout = { explorer_visible: true, explorer_width: 280, collapsed: [] };
export function Dashboard({ api, projects, transportFailed = false, environments, selection, layout = defaultLayout, onSelection, onLayout, remove, retry, relink, view, onView, onWorktreeRemoved, addProjectControl }: {
  api: ApiClient; projects: Project[]; transportFailed?: boolean; environments?: EnvironmentView[];
  selection?: Selection | null; layout?: Layout; onSelection?: (value: Selection | null) => void; onLayout?: (value: Layout) => void;
  remove?: (project: Project) => Promise<void>; retry?: (env: ExecutionEnvironment) => void; relink?: (env: ExecutionEnvironment) => void;
  onWorktreeRemoved?: (selection: Selection, layout: Layout) => void;
  view?: 'overview' | 'changes' | 'terminal'; onView?: (value: 'overview' | 'changes' | 'terminal') => void;
  addProjectControl?: ReactNode;
}) {
  const cache = useQueryClient();
  const [removeDialog, setRemoveDialog] = useState<{ tree: Worktree; context: string } | null>(null);
  const [creating, setCreating] = useState<{ context: string } | null>(null);
  const [internal, setInternal] = useState<Selection | null>(null);
  const selected = selection === undefined ? internal : selection;
  const currentContext = useRef('');
  currentContext.current = JSON.stringify(selected);
  const choose = (value: Selection | null) => { setCreating(null); setRemoveDialog(null); if (onSelection) onSelection(value); else setInternal(value); };
  const [actionError, setActionError] = useState('');
  const [internalTab, setInternalTab] = useState<'overview' | 'changes' | 'terminal'>('overview');
  const tab = (view ?? internalTab)==='terminal'?'overview':(view ?? internalTab);
  const setTab = onView ?? setInternalTab;
  const [removing, setRemoving] = useState<string | null>(null);
  const [refreshing,setRefreshing]=useState<string|null>(null);
  const refreshBusy=useRef(false);
  async function refreshProject(project:Project){
    if(refreshBusy.current)return;refreshBusy.current=true;setRefreshing(JSON.stringify([project.environment_id,project.id]));setActionError('');
    const queryKey=worktreeCacheKey(project.environment_id,project.id);
    try{await cache.cancelQueries({queryKey});const snapshot=await api.worktrees(project.id,undefined,project.environment_id,true);cache.setQueryData(queryKey,snapshot);if(snapshot.error)setActionError(snapshot.error.message);}
    catch(e){setActionError((e as Error).message);}finally{refreshBusy.current=false;setRefreshing(null);}
  }
  const canRemove=(tree:Worktree)=>desktopMode()&&!failedEnv(tree.environment_id)&&!tree.is_main&&!tree.is_bare&&!tree.is_locked&&(tree.is_available||tree.is_prunable);
  const openRemove=(tree:Worktree)=>setRemoveDialog({tree,context:currentContext.current});
  const [now, setNow] = useState(Date.now);
  const [previewWidth, setPreviewWidth] = useState<number | null>(null);
  const explorerWidth = previewWidth ?? layout.explorer_width;
  useEffect(() => { if (!layout.explorer_visible) setPreviewWidth(null); }, [layout.explorer_visible]);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  const envs: EnvironmentView[] = environments ?? [...new Set(projects.map(p => p.environment_id))].map(id => ({ environment: { id, kind: 'wsl' as const, distro: id }, state: transportFailed ? 'disconnected' : 'connected', error: null, authoritative: true }));
  const failedEnv = (id: string) => transportFailed || envs.find(e => e.environment.id === id)?.state !== 'connected' || !!envs.find(e => e.environment.id === id)?.projects_error;
  const queries = useQueries({ queries: projects.map(project => ({
    queryKey: worktreeCacheKey(project.environment_id, project.id),
    queryFn: ({ signal }: { signal: AbortSignal }) => api.worktrees(project.id, signal, project.environment_id),
    enabled: !failedEnv(project.environment_id), refetchInterval: 5000,
  })) });
  const selectedIndex = projects.findIndex(p => p.environment_id === selected?.environment_id && p.id === selected.project_id);
  const selectedProject = projects[selectedIndex];
  const selectedQuery = queries[selectedIndex];
  const selectedTree = selectedQuery?.data?.worktrees.find(t => t.id === selected?.worktree_id);
  const selectedEnv = envs.find(e => e.environment.id === selected?.environment_id);
  useEffect(() => {
    if (!selected) return;
    if (selectedEnv?.authoritative && !failedEnv(selected.environment_id) && selectedIndex === -1) choose(null);
    else if (selected.worktree_id && selectedQuery?.data && !selectedQuery.isError && !selectedQuery.data.error && !failedEnv(selected.environment_id) && !selectedTree) choose({ ...selected, worktree_id: null });
  }, [selected, selectedIndex, selectedQuery?.data, selectedQuery?.isError, selectedTree, selectedEnv?.authoritative, selectedEnv?.state]);
  const toggle = (id: string) => onLayout?.({ ...layout, collapsed: layout.collapsed.includes(id) ? layout.collapsed.filter(v => v !== id) : [...layout.collapsed, id] });
  async function unregister(project: Project) {
    setRemoving(JSON.stringify([project.environment_id, project.id])); setActionError('');
    try { await remove?.(project); if (selected?.environment_id === project.environment_id && selected.project_id === project.id) choose(null); }
    catch (e) { setActionError((e as Error).message); } finally { setRemoving(null); }
  }
  const agentView = desktopMode() && tab === 'overview' && !!selectedTree;
  const repositoryInfo = <>
          <code>{selectedTree?.path ?? selectedProject?.root_path}</code>
          {desktopMode() && selectedTree && <ProviderCheck key={`provider:${key(selectedTree)}`} tree={selectedTree} connected={!failedEnv(selectedTree.environment_id) && !selectedQuery?.isError && !selectedQuery?.data?.error && selectedTree.is_available}/>}
          {selectedTree && selectedQuery?.data && <><TreeFlags tree={selectedTree}/><dl><dt>HEAD</dt><dd><code>{selectedTree.head ?? 'Нет commit (unborn)'}</code></dd><dt>Доступность</dt><dd>{failedEnv(selectedTree.environment_id) || selectedQuery.isError || selectedQuery.data.error ? 'Не подтверждена; last-known observation' : selectedTree.is_available ? 'Подтверждена при последнем refresh' : 'Недоступно: missing или identity не подтверждена'}</dd><dt>Discovery</dt><dd>{timestamp(selectedQuery.data.last_successful_refresh_at)}</dd></dl>
          {selectedTree.availability_error && <p role="alert">{selectedTree.availability_error.code}: {selectedTree.availability_error.message}</p>}
          <h3>Git status</h3><GitSummary tree={selectedTree} snapshot={selectedQuery.data} failed={failedEnv(selectedTree.environment_id) || selectedQuery.isError} now={now}/></>}
          {selectedProject && <div className="repository-actions">{desktopMode() && selectedTree && <Button variant="outline" disabled={!canRemove(selectedTree)} onClick={()=>openRemove(selectedTree)}>{!selectedTree.is_available&&selectedTree.is_prunable?'Удалить недоступный worktree':'Remove worktree'}</Button>}<Button variant="outline" disabled={refreshing!==null||failedEnv(selectedProject.environment_id)} onClick={() => { void refreshProject(selectedProject); }}>Обновить</Button>{remove && <Button variant="outline" disabled={removing !== null || failedEnv(selectedProject.environment_id)} onClick={() => { void unregister(selectedProject); }}>Убрать проект из списка</Button>}<p className="muted">Удаление проекта из списка сохраняет repository, исходники и worktree.</p></div>}
</>;
  return <div className={`dashboard${previewWidth !== null ? ' resizing' : ''}`} style={{ gridTemplateColumns: layout.explorer_visible ? `${explorerWidth}px ${onLayout ? '5px ' : ''}minmax(0, 1fr)` : 'minmax(0, 1fr)' }}>
    {removeDialog && <RemoveWorktree api={api} tree={removeDialog.tree} close={()=>setRemoveDialog(null)} removed={result=>{
      if(currentContext.current!==removeDialog.context)return;
      if(result.snapshot)cache.setQueryData(worktreeCacheKey(result.environment_id,result.project_id),result.snapshot);
      else void cache.invalidateQueries({queryKey:worktreeCacheKey(result.environment_id,result.project_id)});
      if(result.git_removed){
        const removingSelected=selected?.environment_id===result.environment_id&&selected.project_id===result.project_id&&selected.worktree_id===result.worktree_id;
        const next=removingSelected?{environment_id:result.environment_id,project_id:result.project_id,worktree_id:null}:selected;
        const nextLayout={...layout,terminals:(layout.terminals??[]).filter(t=>!(t.environment_id===result.environment_id&&t.project_id===result.project_id&&t.worktree_id===result.worktree_id))};
        if(onWorktreeRemoved&&next)onWorktreeRemoved(next,nextLayout);
        else {if(removingSelected){if(onSelection)onSelection(next);else setInternal(next);}onLayout?.(nextLayout);}
      }
    }}/>}
    {creating && <CreateWorktree api={api} projects={projects} environments={envs} close={() => setCreating(null)} created={result => {
      if (currentContext.current !== creating.context) return;
      if (result.snapshot) cache.setQueryData(worktreeCacheKey(result.environment_id, result.project_id), result.snapshot);
      else void cache.invalidateQueries({ queryKey: worktreeCacheKey(result.environment_id, result.project_id) });
      if (result.git_created && result.worktree_id) {
        const next = { environment_id: result.environment_id, project_id: result.project_id, worktree_id: result.worktree_id };
        if (onSelection) onSelection(next); else setInternal(next);
      }
    }}/>}
    {layout.explorer_visible && <nav aria-label="Проекты и worktree" className="project-tree">
      <div className="panel-title explorer-heading"><h2>Проекты <span className="section-count">{projects.length}</span></h2>{desktopMode() && <ActionMenu label="Действия с проектами"><button aria-label="Create worktree" onClick={() => setCreating({ context: currentContext.current })}><Icon name="worktree"/>Создать worktree</button></ActionMenu>}</div>
      {envs.map(env => <section className="environment" key={env.environment.id} aria-label={`Environment ${env.environment.distro ?? 'Windows'}`}>
        <div className="environment-heading"><button onClick={() => toggle(`env:${env.environment.id}`)} aria-expanded={!layout.collapsed.includes(`env:${env.environment.id}`)}><Icon name="chevron" className={!layout.collapsed.includes(`env:${env.environment.id}`) ? 'expanded' : ''} size={12}/><Icon name={env.environment.kind === 'windows' ? 'monitor' : 'server'}/>{env.environment.kind === 'windows' ? 'Windows' : env.environment.distro}</button><span className={`connection ${env.state}`} title={env.state}>{env.state === 'connected' ? 'Подключено' : env.state}</span></div>
        {env.error && <p role="alert">Environment: {env.error.code} — {env.error.message}</p>}
        {env.projects_error && <p role="alert">Registry: {env.projects_error}</p>}
        {env.state !== 'connected' && retry && <button className="small-action" onClick={() => retry(env.environment)}>Повторить {env.environment.distro ?? 'Windows'}</button>}
        {env.error?.code === 'identity_mismatch' && relink && <button className="small-action" onClick={() => relink(env.environment)}>Явно перепривязать {env.environment.distro}</button>}
        {!layout.collapsed.includes(`env:${env.environment.id}`) && <ul className="projects">{projects.map((project, index) => {
          if (project.environment_id !== env.environment.id) return null;
          const query = queries[index]; const snapshot = query.data; const failed = failedEnv(project.environment_id) || query.isError;
          const repoKey = `repo:${project.environment_id}:${project.id}`;
          return <ExplorerItem className="project" key={repoKey} label={`Действия с проектом ${project.name}`} actions={<>
            <button disabled={refreshing!==null||failedEnv(project.environment_id)} onClick={()=>void refreshProject(project)}><Icon name="refresh"/>Обновить</button>
            {remove&&<button disabled={removing!==null||failedEnv(project.environment_id)} onClick={()=>void unregister(project)}><Icon name="close"/>Убрать проект из списка</button>}
          </>}>
            <div className="project-heading"><button className="disclosure" aria-label={`Раскрыть ${project.name}`} aria-expanded={!layout.collapsed.includes(repoKey)} onClick={() => toggle(repoKey)}><Icon name="chevron" className={!layout.collapsed.includes(repoKey) ? 'expanded' : ''} size={12}/></button><button className="repository-select" onClick={() => choose({ environment_id: project.environment_id, project_id: project.id, worktree_id: null })} title={project.root_path}><Icon name="folder"/>{project.name}</button></div>
            {!layout.collapsed.includes(repoKey) && <>
              {query.isPending && !failed && <p role="status">Loading — worktree…</p>}
              {query.error && <p role="alert">Ошибка проекта: {query.error.message}</p>}
              {snapshot?.error && <p role="alert">Discovery error: {snapshot.error.code} — {snapshot.error.message}</p>}
              {snapshot && (failed || snapshot.error) && <p className="discovery-time">Discovery: {timestamp(snapshot.last_successful_refresh_at)} · last-known</p>}
              {snapshot?.worktrees.length === 0 && !snapshot.error && <p className="muted">Worktree не обнаружены.</p>}
              <ul className="worktrees">{snapshot?.worktrees.map(tree => {
                const view = gitView(tree, snapshot, failed, now);
                return <ExplorerItem key={key(tree)} className="worktree-row" label={`Действия с worktree ${treeName(tree)}`} actions={<>
                  <button disabled={refreshing!==null||failedEnv(project.environment_id)} onClick={()=>void refreshProject(project)}><Icon name="refresh"/>Обновить</button>
                  {desktopMode()&&<button disabled={!canRemove(tree)||!!snapshot.error||query.isError} title={tree.is_main?'Основной worktree нельзя удалить':tree.is_locked?'Сначала разблокируйте worktree в Git':undefined} onClick={()=>openRemove(tree)}><Icon name="close"/>{!tree.is_available&&tree.is_prunable?'Удалить недоступный worktree…':'Удалить worktree…'}</button>}
                </>}><button className="worktree-select" title={tree.path} aria-pressed={selected?.environment_id === tree.environment_id && selected.project_id === tree.project_id && selected.worktree_id === tree.id} onClick={() => choose({ environment_id: tree.environment_id, project_id: tree.project_id, worktree_id: tree.id })}>
                  <span className="tree-title"><Icon name="worktree" size={14}/><strong>{treeName(tree)}</strong>{tree.is_main && <span className="badge">Primary</span>}</span>
                  <span className="tree-secondary"><span className="branch-label" title={branchLabel(tree)}><Icon name="branch" size={12}/>{branchLabel(tree)}</span>
                  <span className={`tree-status ${view.current ? 'current' : 'stale'}`}>{view.label}{view.current && tree.git_status.files.length > 0 ? ` · ${new Set(tree.git_status.files.map(f => f.path)).size} files` : ''}</span></span>
                  {(tree.is_bare || tree.is_locked || tree.is_prunable || !tree.is_available) && <span className="tree-flags">{[tree.is_bare && 'Bare', tree.is_locked && 'Locked', tree.is_prunable && 'Prunable', !tree.is_available && 'Missing'].filter(Boolean).join(' · ')}</span>}
                </button></ExplorerItem>;
              })}</ul>
            </>}
          </ExplorerItem>;
        })}</ul>}
        {env.loading && <p role="status">Loading — registry…</p>}
        {(env.projects_loaded ?? env.authoritative) && !projects.some(p => p.environment_id === env.environment.id) && <p className="muted">Нет зарегистрированных проектов</p>}
      </section>)}
      <div className="explorer-footer">{addProjectControl}<button className="workspace-overview" onClick={() => choose(null)}><Icon name="grid"/>Workspace Overview</button></div>
    </nav>}
    {layout.explorer_visible && onLayout && <ExplorerResize width={explorerWidth} preview={setPreviewWidth} commit={width => onLayout({ ...layout, explorer_width: width })}/>}
    <section className={`overview${agentView ? ' agent-overview' : ''}`} aria-label="Worktree overview">
      {actionError && <p role="alert">{actionError}</p>}
      {selected ? <>
        {tab === 'overview' && !agentView && <div className="worktree-header"><h2>{selectedProject?.name ?? selected.project_id} / {selectedTree ? treeName(selectedTree) : selected.worktree_id ? 'Worktree недоступен' : 'Repository'}</h2><span className={`connection ${selectedEnv?.state ?? 'error'}`}>{selectedEnv?.environment.distro ?? 'Windows'} · {selectedEnv?.state ?? 'Unknown'}</span>
        {selectedTree && <p className="context-meta"><span><Icon name="branch" size={14}/> {branchLabel(selectedTree)}</span><code>{commitLabel(selectedTree)}</code></p>}</div>}
        {!onView && <div className="context-tabs"><button className={tab === 'overview' ? 'active-tab' : ''} onClick={() => setTab('overview')}>Overview</button><button className={tab === 'changes' ? 'active-tab' : ''} disabled={!selected.worktree_id} onClick={() => setTab('changes')}>Changes</button></div>}
        {(!selectedProject || failedEnv(selected.environment_id) || selectedQuery?.error || selectedQuery?.data?.error || (selected.worktree_id && !selectedTree && !selectedQuery?.isPending)) && <p role="alert">Выбранный контекст недоступен. {selectedEnv?.error?.message ?? selectedEnv?.projects_error ?? selectedQuery?.error?.message ?? selectedQuery?.data?.error?.message ?? 'Сохранённые IDs ещё не подтверждены.'}</p>}
        {selectedQuery?.isPending && !failedEnv(selected.environment_id) && <p role="status">Loading — выбранный Worktree…</p>}
        {selectedProject && selectedTree && <>
        <WorkspaceView key={`changes:${key(selectedTree)}`} active={tab === 'changes'}><Changes api={api} project={selectedProject} tree={selectedTree} connected={!failedEnv(selected.environment_id)} unavailable={selectedQuery?.error?.message ?? selectedQuery?.data?.error?.message ?? selectedTree.availability_error?.message ?? (!selectedTree.is_available ? 'Worktree missing / identity не подтверждена' : null)}/></WorkspaceView>

        </>}
        {(!selectedTree || (tab === 'overview' && !agentView)) && <div className="overview-content">{repositoryInfo}</div>}
      </> : <div className="empty-selection workspace-empty"><Icon name="worktree" size={32}/><h2>Выберите worktree</h2><p className="muted">Откройте рабочую копию в Explorer, чтобы просмотреть изменения или продолжить работу с агентом.</p><span className="empty-shortcut"><kbd>Ctrl+Alt+P</kbd> Быстрый переход и команды</span></div>}
    {desktopMode() && projects.flatMap((_project,index)=>(queries[index].data?.worktrees??[]).map(tree=>{
      const query=queries[index],active=tab==='overview'&&selected?.environment_id===tree.environment_id&&selected.project_id===tree.project_id&&selected.worktree_id===tree.id;
      return <WorkspaceView key={`agent:${key(tree)}`} active={active}><HarnessPanel api={api} tree={tree} connected={!failedEnv(tree.environment_id)&&!query.isError&&!query.data?.error&&tree.is_available}
        layout={layout.terminals?.find(t=>t.environment_id===tree.environment_id&&t.project_id===tree.project_id&&t.worktree_id===tree.id)}
        saveLayout={value=>onLayout?.({...layout,terminals:[...(layout.terminals??[]).filter(t=>!(t.environment_id===value.environment_id&&t.project_id===value.project_id&&t.worktree_id===value.worktree_id)),value]})}
        diagnostics={active?repositoryInfo:undefined}/></WorkspaceView>;
    }))}
    </section>
  </div>;
}





