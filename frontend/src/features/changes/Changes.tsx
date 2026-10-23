import { Component, lazy, Suspense, useEffect, useRef, useState, type ReactNode } from 'react';
import { ApiError, type ApiClient, type DiffRequest, type DiffResponse, type Project, type Worktree } from '../../api';
import { fresh, timestamp } from '../dashboard/status';
import { ExplorerResize } from '../dashboard/ExplorerResize';
import { Icon } from '../../components/ui/icon';
import { WorkingFile } from './WorkingFile';
import { allDrafts, draftRevision, subscribeDrafts, takeRequestedDraft } from './drafts';
import { useSyncExternalStore } from 'react';
import { ActionMenu } from '../../components/ui/action-menu';
const MonacoPatch = lazy(() => import('./MonacoPatch'));
class EditorBoundary extends Component<{ children: ReactNode }, { error: string }> {
  state = { error: '' };
  static getDerivedStateFromError(error: Error) { return { error: error.message }; }
  render() { return this.state.error ? <p role="alert" className="diff-message">Monaco error: {this.state.error}</p> : this.props.children; }
}
type File = DiffResponse['files'][number];
const fallback = {
  bare: 'Bare repository: working tree отсутствует.', unborn: 'Unborn: HEAD ещё не содержит commit. Доступны initial staged/untracked файлы.',
  no_base: 'No base: укажите существующий связанный base ref.', ambiguous_base: 'Неоднозначный merge-base: branch diff недоступен.',
  binary: 'Binary: текстовый diff недоступен.', large_file: 'Large file: patch превышает лимит 1 MiB.',
  conflict: 'Conflict: unmerged файл. Разрешение конфликта здесь недоступно.', submodule: 'Submodule: изменение ссылки/состояния вложенного repository.',
};
// Cancellation is advisory for IPC. The active flag also discards late completions.
function useDiff(api: ApiClient, environment: string, request: DiffRequest, enabled: boolean, revision: number, source: string): { data?: DiffResponse; error?: Error; loading: boolean } {
  const scope = JSON.stringify([environment, request]);
  const [value, setValue] = useState<{ scope: string; source: string; revision: number; available: boolean; data?: DiffResponse; error?: Error; loading: boolean }>();
  useEffect(() => {
    if (!enabled) { setValue(old => old ? { ...old, available: false, loading: false } : old); return; }
    let active = true; const controller = new AbortController();
    setValue(old => ({ scope, source, revision, available: true, data: old?.scope === scope ? old.data : undefined, loading: true }));
    void api.diff(environment, request, controller.signal).then(data => {
      if (active) setValue({ scope, source, revision, available: true, data, loading: false });
    }, error => { if (active) setValue(old => ({ scope, source, revision, available: true, data: old?.scope === scope ? old.data : undefined, error, loading: false })); });
    return () => { active = false; controller.abort(); };
  }, [api, scope, enabled, revision, source]);
  if (value?.scope !== scope) return { loading: enabled };
  const pending = enabled && (!value.available || value.revision !== revision || value.source !== source);
  return { ...value, error: pending ? undefined : value.error, loading: pending || (enabled && value.loading) };
}
function Counts({ file }: { file: File }) {
  return <span className="file-counts"><span className="additions">+{file.additions ?? '—'}</span><span className="deletions">−{file.deletions ?? '—'}</span></span>;
}
export function Changes({ api, project, tree, connected, unavailable }: { api: ApiClient; project: Project; tree: Worktree; connected: boolean; unavailable: string | null }) {
  const body = useRef<HTMLDivElement>(null);
  const [bodyWidth, setBodyWidth] = useState(1000);
  const [filesWidth, setFilesWidth] = useState(270);
  const [widthPreview, setWidthPreview] = useState<number | null>(null);
  const maxWidth = Math.max(180, Math.min(520, Math.floor(bodyWidth) - 245));
  const width = Math.min(widthPreview ?? filesWidth, maxWidth);
  useEffect(() => {
    if (!body.current || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => setBodyWidth(entry.contentRect.width));
    observer.observe(body.current);
    return () => observer.disconnect();
  }, []);
  const [mode, setMode] = useState<DiffRequest['mode']>('unstaged');
  const [base, setBase] = useState(project.base_ref ?? '');
  const [draft, setDraft] = useState(base);
  const [fileId, setFileId] = useState<string | null>(null);
  const [context, setContext] = useState(3);
  const [revision, setRevision] = useState(0);
  useSyncExternalStore(subscribeDrafts, draftRevision);
  const [editingFile, setEditingFile] = useState<File>();
  useEffect(() => {
    const saved = () => setRevision(value => value + 1);
    window.addEventListener('changes-saved', saved);
    return () => window.removeEventListener('changes-saved', saved);
  }, []);
  const [sideBySide, setSideBySide] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(timer); }, []);
  const request: DiffRequest = { project_id: project.id, worktree_id: tree.id, mode, base_ref: mode === 'branch' ? base || null : null, file_id: null, context_lines: 3 };
  const enabled = connected && !unavailable;
  // Exclude polling timestamps: unchanged Explorer observations never reload patches.
  const source = JSON.stringify([tree.head, tree.branch, tree.is_available, tree.availability_error, tree.git_status?.error,
    tree.git_status?.staged, tree.git_status?.unstaged, tree.git_status?.untracked, tree.git_status?.conflicts, tree.git_status?.files]);
  const list = useDiff(api, tree.environment_id, request, enabled, revision, source);
  const scopedDrafts = allDrafts().filter(d => d.environment === tree.environment_id && d.request.project_id === project.id && d.request.worktree_id === tree.id);
  const visibleFiles = [...(list.data?.files ?? [])];
  if (mode === 'unstaged') for (const draft of scopedDrafts) {
    if (!visibleFiles.some(f => f.path === draft.request.path)) visibleFiles.push({ id: draft.key, path: draft.request.path, original_path: null, status: 'M', additions: draft.status === 'saved' ? 0 : null, deletions: draft.status === 'saved' ? 0 : null, binary: false, submodule: false });
  }
  useEffect(() => {
    const open = () => {
      const target = takeRequestedDraft(tree.environment_id, tree.id);
      if (target) {
        setMode('unstaged');
        setFileId(list.data?.files.find(f => f.path === target.request.path)?.id ?? target.key);
      }
    };
    window.addEventListener('changes-open-draft', open);
    open();
    return () => window.removeEventListener('changes-open-draft', open);
  }, [tree.environment_id, tree.id, list.data]);
  const retained = editingFile && allDrafts().some(d => d.environment === tree.environment_id && d.request.worktree_id === tree.id && d.request.path === editingFile.path);
  const selected = visibleFiles.find(f => f.id === fileId) ?? (mode === 'unstaged' && retained && editingFile?.id === fileId ? visibleFiles.find(f => f.path === editingFile.path) : undefined);
  const detail = useDiff(api, tree.environment_id, { ...request, file_id: selected?.id ?? null, context_lines: context }, enabled && !!selected && !!list.data?.files.some(f => f.id === selected.id) && !list.loading && !list.error, revision, source);
  const data = detail.data ?? list.data;
  const file = detail.data?.files.find(f => f.id === selected?.id) ?? selected;
  const error = list.error ?? detail.error;
  const observed = data?.observed_at;
  const stale = !enabled || !!error || list.loading || detail.loading || !fresh(observed ?? null, now) ||
    (!!tree.git_status && !tree.is_bare && (!fresh(tree.git_status.last_successful_refresh_at, now) || !!tree.git_status.error));
  const selectedPatch = detail.data?.selected_file;
  const editable = mode === 'unstaged' && selected && !selected.submodule && !['U', 'D'].includes(selected.status) && selected.binary !== true && (!selectedPatch || selectedPatch.state === 'text');
  const recoverDraft = mode === 'unstaged' && selected && scopedDrafts.some(d => d.request.path === selected.path && d.status !== 'saved');
  const changeMode = (value: DiffRequest['mode']) => { setMode(value); setFileId(null); setContext(3); };
  return <section className="changes" aria-label="Changes screen">
    <div className="changes-toolbar"><div className="diff-modes">{(['unstaged', 'staged', 'branch'] as const).map(value => <button key={value} aria-pressed={mode === value} onClick={() => changeMode(value)}>{value === 'branch' ? 'Branch vs base' : value === 'staged' ? 'Staged' : 'Unstaged'}</button>)}</div>
      {mode === 'branch' && <form className="base-form" onSubmit={e => { e.preventDefault(); setBase(draft.trim()); setFileId(null); setContext(3); }}><label htmlFor="diff-base">Base</label><input id="diff-base" placeholder="main / ref / commit" value={draft} maxLength={256} onChange={e => setDraft(e.target.value)}/><button type="submit">Применить</button></form>}
      <div className={`diff-observation${stale ? ' observation-warning' : ''}`} role="status" title={observed ? `Проверено: ${timestamp(observed)}` : undefined}>{!connected ? 'Disconnected — diff не подтверждён.' : unavailable ? `Недоступно: ${unavailable}` : error ? `${error instanceof ApiError && error.kind === 'disconnected' ? 'Disconnected' : 'Error'} — ${error.message}` : list.loading ? 'Loading — список файлов…' : detail.loading ? 'Loading — выбранный diff…' : stale ? 'Stale — перечитайте Changes.' : mode === 'unstaged' ? 'Рабочие файлы · автосохранение' : 'Только чтение: сравнение с Git index/HEAD'}{stale && observed && ` · ${timestamp(observed)}`}{stale && data && ' · last-known observation'}</div>
      <div className="changes-actions">
        <label className="diff-view-select"><Icon name={sideBySide ? 'split' : 'rows'}/><select aria-label="Вид diff" value={sideBySide ? 'split' : 'inline'} onChange={event => setSideBySide(event.target.value === 'split')}><option value="inline">Inline</option><option value="split">Side-by-side</option></select></label>
        <button className={`icon-button${list.loading ? ' is-loading' : ''}`} title="Обновить изменения" aria-label="Refresh Changes" disabled={!enabled || list.loading} onClick={() => setRevision(r => r + 1)}><Icon name="refresh"/></button>
        <ActionMenu label="Параметры diff"><button aria-label="Расширить контекст" title={`Контекст: ${context} строк · только hunks`} disabled={!selected || !enabled || list.loading || !!error || detail.loading || selectedPatch?.state !== 'text' || context >= 100} onClick={() => setContext(c => Math.min(100, c + 20))}><Icon name="expand"/>Расширить контекст<span className="menu-hint">+20 строк</span></button><p className="menu-empty">Рабочий diff использует полный файл. Автосохранение не меняет Git index.</p></ActionMenu>
      </div>
    </div>
    {data?.state && data.state !== 'ready' && <p className="diff-message">{fallback[data.state]}</p>}
    <div className="changes-body" ref={body} style={{ gridTemplateColumns: `${width}px 5px minmax(0, 1fr)` }}>
      <nav className="changed-files" aria-label="Changed Files"><div className="panel-title"><h3>Changed Files</h3><span>{list.data ? list.data.files.length : '—'}{stale && list.data ? ' · last-known' : ''}</span></div>
        {visibleFiles.map(item => <button key={item.id} className="changed-file" aria-pressed={item.id === fileId} onClick={() => { setFileId(item.id); setEditingFile(item); setContext(3); }} title={item.original_path ? `${item.original_path} → ${item.path}` : item.path}>
          <span className="file-title"><Icon name="file" size={15}/><strong>{item.path.split('/').pop()}</strong><Counts file={file?.id === item.id ? file : item}/><span className={`file-status status-${item.status}`}>{item.status === '?' ? 'A' : item.status}</span></span>
          <span className="file-directory">{item.path.includes('/') ? item.path.slice(0, item.path.lastIndexOf('/')) : '/'}</span>
          {item.original_path && <span className="file-directory">{item.original_path} → {item.path}</span>}
          {(item.status === '?' || item.binary || item.submodule || item.status === 'U') && <span className="file-directory">{item.binary ? 'Binary' : item.status === '?' ? (item.additions === null ? 'Untracked · статистика недоступна' : 'Untracked') : item.submodule ? 'Submodule' : 'Conflict'}</span>}
        </button>)}
        {!list.loading && !error && visibleFiles.length === 0 && list.data && <p className="diff-message">{!stale && list.data.state === 'ready' ? 'Нет изменений в выбранном режиме.' : 'Нет доступных файлов diff.'}</p>}
      </nav>
      <ExplorerResize label="Ширина Changed Files" width={width} min={180} max={maxWidth} reset={270} preview={setWidthPreview} commit={setFilesWidth}/>
      <div className="diff-panel"><div className="diff-file-header"><Icon name="file" size={14}/><code title={file?.path}>{file?.path ?? 'Выберите файл'}</code>{file && <Counts file={file}/>}</div>
        <div className="diff-scroll">
          {!selected && <p className="diff-message">{fileId && list.data && !list.loading && !list.error ? 'Выбранный файл больше не входит в текущий diff. Выберите другой файл.' : 'Выберите файл для загрузки его diff.'}</p>}
          {!editable && selectedPatch && selectedPatch.state !== 'text' && <p className="diff-message">{fallback[selectedPatch.state]}</p>}
          {!editable && selectedPatch?.state === 'text' && selectedPatch.patch !== null && file && <EditorBoundary key={file.id}><Suspense fallback={<p className="diff-message">Loading — Monaco…</p>}><MonacoPatch patch={selectedPatch.patch} path={file.path} sideBySide={sideBySide}/></Suspense></EditorBoundary>}
          {(editable || recoverDraft) && selected && <WorkingFile key={JSON.stringify([tree.environment_id, tree.id, selected.path])} api={api} environment={tree.environment_id} request={{ project_id: project.id, worktree_id: tree.id, path: selected.path, text: null, version: null }} sideBySide={sideBySide} connected={enabled && !!editable} observation={list.data?.observed_at} fallback={selectedPatch?.patch != null ? <EditorBoundary><Suspense><MonacoPatch patch={selectedPatch.patch} path={selected.path} sideBySide={sideBySide}/></Suspense></EditorBoundary> : null}/>}
          {!editable && selected && <p className="diff-message">Только чтение: {mode !== 'unstaged' ? 'сравнение с Git index/HEAD' : selected.status === 'D' ? 'файл удалён' : selected.status === 'U' ? 'Git conflict' : 'submodule'}.</p>}
          {!editable && selected && !selectedPatch && !detail.loading && !error && <p className="diff-message">Diff не получен. Перечитайте Changes.</p>}
        </div>
      </div>
    </div>
  </section>;
}
