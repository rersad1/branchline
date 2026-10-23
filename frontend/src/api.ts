import { command, desktopMode, type Selection, type Layout } from './desktop';
export interface ApiProblem { code: string; message: string }
export interface ExecutionEnvironment { id: string; kind: 'windows' | 'wsl'; distro: string | null }
export interface DirectoryPage { environment_id: string; path: string; parent: string | null; entries: { name: string; path: string; symlink: boolean }[]; next_offset: number | null }
export const worktreeCacheKey = (environmentId: string, projectId: string) => ['worktrees', environmentId, projectId] as const;
export interface Project {
  environment_id: string;
  id: string; name: string; root_path: string; git_common_dir: string;
  base_ref: string | null; created_at: string;
}
export interface FileStatus {
  path: string; original_path: string | null; index_status: string; worktree_status: string;
  kind: 'ordinary' | 'renamed' | 'copied' | 'unmerged' | 'untracked'; similarity: number | null;
  submodule: { commit_changed: boolean; modified: boolean; untracked: boolean } | null;
}
export interface GitStatus {
  staged: number | null; unstaged: number | null; untracked: number | null; conflicts: number | null;
  files: FileStatus[]; last_successful_refresh_at: string | null; error: ApiProblem | null;
}
export interface Worktree {
  environment_id: string;
  id: string; project_id: string; path: string; branch: string | null; head: string | null;
  is_main: boolean; is_bare: boolean; is_detached: boolean; is_locked: boolean;
  lock_reason: string | null; is_prunable: boolean; prune_reason: string | null;
  is_available: boolean; availability_error: ApiProblem | null; git_status: GitStatus;
}
export interface Snapshot {
  environment_id: string;
  project_id: string; worktrees: Worktree[]; last_successful_refresh_at: string | null; error: ApiProblem | null;
}
export interface RemoveRequest { project_id: string; worktree_id: string; confirm_path: string; external_sessions_checked: boolean; missing_only?: boolean }
export interface RemoveResponse { environment_id: string; project_id: string; worktree_id: string; git_removed: boolean; path_exists: boolean | null; snapshot: Snapshot | null; warning: string | null }
export type TerminalAction = { action: 'open_ide'; request_id: string; executable: string } | { action: 'open_harness'; request_id: string; rows: number; cols: number; installation_id: string } | { action: 'open'; request_id: string; rows: number; cols: number } | { action: 'list' } | { action: 'read'; session_id: string; cursor: number } | { action: 'input'; session_id: string; data: string } | { action: 'resize'; session_id: string; rows: number; cols: number } | { action: 'close'; session_id: string };
export type TerminalRequest = TerminalAction & { project_id: string; worktree_id: string };
export interface TerminalSession { id: string; project_id: string; worktree_id: string; cwd: string; shell: string; state: 'running' | 'exited' | 'error'; exit_code: number | null; error: string | null; rows: number; cols: number }
export interface TerminalResponse { environment_id: string; project_id: string; worktree_id: string; sessions: TerminalSession[]; session_id: string | null; data: number[]; cursor: number; lost: boolean }
export interface CreateRequest { project_id: string; base_ref: string; branch: string; path: string }
export interface CreateResponse { environment_id: string; project_id: string; path: string; branch: string; git_created: boolean; branch_exists: boolean | null; path_exists: boolean | null; worktree_id: string | null; snapshot: Snapshot | null; warning: string | null }
export interface DiffRequest { project_id: string; worktree_id: string; mode: 'unstaged' | 'staged' | 'branch'; base_ref: string | null; file_id: string | null; context_lines: number }
export interface DiffResponse {
  environment_id: string; project_id: string; worktree_id: string; mode: DiffRequest['mode']; base_ref: string | null; base_commit: string | null; head_commit: string | null;
  state: 'ready' | 'unborn' | 'bare' | 'no_base' | 'ambiguous_base'; context_lines: number; observed_at: string;
  files: { id: string; path: string; original_path: string | null; status: string; additions: number | null; deletions: number | null; binary: boolean | null; submodule: boolean }[];
  selected_file: { file_id: string; state: 'text' | 'binary' | 'large_file' | 'conflict' | 'submodule'; patch: string | null } | null;
}
export class ApiError extends Error {
  constructor(public readonly kind: 'error' | 'disconnected', public readonly status?: number, public readonly code?: string) {
    super(kind === 'disconnected' ? 'Rust backend недоступен. Проверьте запуск и соединение.' :
      status === 401 ? 'Авторизация отклонена. Введите токен текущего backend.' :
      status === 403 ? 'Origin отклонён. Откройте UI через локальный gateway.' :
      `Ошибка API${status ? ` (HTTP ${status})` : ''}. Повторите запрос.`);
  }
}
export interface EditRequest { project_id: string; worktree_id: string; path: string; version: string | null; text: string | null }
export interface EditResponse { environment_id: string; project_id: string; worktree_id: string; path: string; version: string; text: string; original: string }
export class ApiClient {
  async editFile(environment: string, request: EditRequest): Promise<EditResponse> {
    let result: EditResponse;
    if (desktopMode()) result = await this.ipc(() => command('edit_file', { scope: { environment_id: environment }, request }));
    else {
      const response = await fetch(`/api/v1/edit?environment_id=${encodeURIComponent(environment)}`, { method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(request), credentials: 'omit', cache: 'no-store', redirect: 'error' });
      if (!response.ok) { const body = await response.json(); const error = new ApiError('error', response.status, body.error?.code); error.message = body.error?.message ?? error.message; throw error; }
      result = await response.json();
    }
    if (result.environment_id !== environment || result.project_id !== request.project_id || result.worktree_id !== request.worktree_id || result.path !== request.path) throw new ApiError('error');
    return result;
  }

  async harnesses(environmentId:string,request:import('./desktop').HarnessRequest,signal?:AbortSignal):Promise<import('./desktop').HarnessResponse>{
    const r=await this.ipc(()=>command('harnesses',{scope:{environment_id:environmentId},request},signal));
    if(r.environment_id!==environmentId||r.request_id!==request.request_id||r.installations.some(i=>i.environment_id!==environmentId))throw new ApiError('error');return r;
  }
  async agents(environmentId:string,request:import('./desktop').AgentRequest,signal?:AbortSignal):Promise<import('./desktop').AgentResponse>{
    const r=await this.ipc(()=>command('agents',{scope:{environment_id:environmentId},request},signal));
    if(r.environment_id!==environmentId||r.project_id!==request.project_id||r.worktree_id!==request.worktree_id||r.request_id!==request.request_id||r.instances.some(i=>i.environment_id!==environmentId||i.project_id!==request.project_id||i.worktree_id!==request.worktree_id))throw new ApiError('error');return r;
  }
  constructor(private readonly token: string) {}
  private async request<T>(path: string, signal?: AbortSignal, method = 'GET', protectedRoute = true): Promise<T> {
    let response: Response;
    try {
      response = await fetch(`/api/v1/${path}`, {
        method, signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(15000)]) : AbortSignal.timeout(15000),
        headers: protectedRoute ? { Authorization: `Bearer ${this.token}` } : {},
        credentials: 'omit', cache: 'no-store', redirect: 'error',
      });
    } catch {
      if (signal?.aborted) throw new DOMException('Cancelled', 'AbortError');
      throw new ApiError('disconnected');
    }
    if (!response.ok) throw new ApiError(response.status >= 502 ? 'disconnected' : 'error', response.status);
    try { return await response.json() as T; } catch { throw new ApiError('error'); }
  }
  private async ipc<T>(operation: () => Promise<T>): Promise<T> {
    try { return await operation(); } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') throw error;
      const problem = error as ApiProblem;
      const result = new ApiError(['disconnected', 'unknown_environment', 'startup_failed'].includes(problem?.code) ? 'disconnected' : 'error', undefined, problem?.code);
      result.message = problem?.message ?? 'Ошибка desktop runtime.';
      throw result;
    }
  }
  health(signal?: AbortSignal) { return desktopMode() ? this.ipc(async () => { await command('runtime_environment', undefined, signal); return { healthy: true, version: '0.1.0' }; }) : this.request<{ healthy: boolean; version: string }>('health', signal, 'GET', false); }
  environment(signal?: AbortSignal) { return desktopMode() ? this.ipc(() => command('runtime_environment', undefined, signal)) : this.request<ExecutionEnvironment>('environment', signal); }
  async diff(environmentId: string, request: DiffRequest, signal?: AbortSignal): Promise<DiffResponse> {
    let result: DiffResponse;
    if (desktopMode()) result = await this.ipc(() => command('read_diff', { scope: { environment_id: environmentId }, request }, signal));
    else {
      let response: Response;
      try {
        response = await fetch(`/api/v1/diff?environment_id=${encodeURIComponent(environmentId)}`, { method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(request), signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(15000)]) : AbortSignal.timeout(15000), credentials: 'omit', cache: 'no-store', redirect: 'error' });
      } catch {
        if (signal?.aborted) throw new DOMException('Cancelled', 'AbortError');
        throw new ApiError('disconnected');
      }
      if (!response.ok) throw new ApiError(response.status >= 502 ? 'disconnected' : 'error', response.status);
      try { result = await response.json() as DiffResponse; } catch { throw new ApiError('error'); }
    }
    if (result.environment_id !== environmentId || result.project_id !== request.project_id || result.worktree_id !== request.worktree_id || result.mode !== request.mode || result.base_ref !== request.base_ref || result.context_lines !== request.context_lines || ((result.selected_file?.file_id ?? null) !== request.file_id && !(result.selected_file === null && result.files.length === 0 && result.state !== 'ready'))) throw new ApiError('error');
    return result;
  }
  async terminal(environmentId: string, request: TerminalRequest): Promise<TerminalResponse> {
    const result = await this.ipc(() => command('terminal', { scope: { environment_id: environmentId }, request }));
    const expected = (request.action === 'open' || request.action === 'open_harness' || request.action === 'open_ide') ? request.request_id : 'session_id' in request ? request.session_id : null;
    if (result.environment_id !== environmentId || result.project_id !== request.project_id || result.worktree_id !== request.worktree_id || result.session_id !== expected || result.sessions.some(s => s.project_id !== request.project_id || s.worktree_id !== request.worktree_id) || result.data.length > 32768 || !Number.isSafeInteger(result.cursor) || result.cursor < 0) throw new ApiError('error');
    return result;
  }
  async removeWorktree(environmentId: string, request: RemoveRequest): Promise<RemoveResponse> {
    const result = await this.ipc(() => command('remove_worktree', { scope: { environment_id: environmentId }, request }));
    if (result.environment_id !== environmentId || result.project_id !== request.project_id || result.worktree_id !== request.worktree_id || (result.snapshot && (result.snapshot.environment_id !== environmentId || result.snapshot.project_id !== request.project_id || result.snapshot.worktrees.some(t => t.environment_id !== environmentId || t.project_id !== request.project_id)))) throw new ApiError('error');
    return result;
  }
  async createWorktree(environmentId: string, request: CreateRequest): Promise<CreateResponse> {
    const result = await this.ipc(() => command('create_worktree', { scope: { environment_id: environmentId }, request }));
    if (result.environment_id !== environmentId || result.project_id !== request.project_id || result.branch !== request.branch ||
      (result.snapshot && (result.snapshot.environment_id !== environmentId || result.snapshot.project_id !== request.project_id || result.snapshot.worktrees.some(t => t.environment_id !== environmentId || t.project_id !== request.project_id))) ||
      (result.worktree_id && !result.snapshot?.worktrees.some(t => t.id === result.worktree_id && t.path === result.path))) throw new ApiError('error');
    return result;
  }
  connectWsl(distro: string, expectedId: string | null = null) { return this.ipc(() => command('connect_wsl', { distro, expectedId })); }
  runtimeStates() { return this.ipc(() => command('runtime_states', undefined)); }
  loadWorkspace() { return this.ipc(() => command('load_workspace', undefined)); }
  saveWorkspace(selection: Selection | null, layout: Layout) { return this.ipc(() => command('save_workspace_ui', { selection, layout })); }
  relinkWsl(environmentId: string) { return this.ipc(() => command('relink_wsl', { environmentId })); }
  async removeProject(environmentId: string, projectId: string) {
    if (desktopMode()) await this.ipc(() => command('remove_project', { scope: { environment_id: environmentId, project_id: projectId } }));
    else {
      const response = await fetch(`/api/v1/projects/${encodeURIComponent(projectId)}?environment_id=${encodeURIComponent(environmentId)}`, { method: 'DELETE', headers: { Authorization: `Bearer ${this.token}` }, signal: AbortSignal.timeout(15000) });
      if (!response.ok) throw new ApiError('error', response.status);
    }
  }
  distros() { return this.ipc(() => command('list_wsl_distros', undefined)); }
  pickWindowsFolder() { return this.ipc(() => command('pick_windows_folder', undefined)); }
  async addProject(environmentId: string, rootPath: string) {
    const project = await this.ipc(() => command('add_project', { scope: { environment_id: environmentId }, rootPath }));
    if (project.environment_id !== environmentId) throw new ApiError('error');
    return project;
  }
  async directories(environmentId: string, path: string, offset = 0) {
    const page = await this.ipc(() => command('browse_directories', { scope: { environment_id: environmentId }, request: { path, offset } }));
    if (page.environment_id !== environmentId) throw new ApiError('error');
    return page;
  }
  async projects(signal?: AbortSignal, environmentId?: string) {
    const projects = desktopMode() ? await this.ipc(() => command('list_projects', { scope: { environment_id: environmentId ?? '' } }, signal)) : await this.request<Project[]>(`projects${environmentId ? `?environment_id=${encodeURIComponent(environmentId)}` : ''}`, signal);
    if (environmentId && projects.some(project => project.environment_id !== environmentId)) throw new ApiError('error');
    return projects;
  }
  async worktrees(projectId: string, signal?: AbortSignal, environmentId?: string, refresh = false) {
    const snapshot = desktopMode() ? await this.ipc(() => command('list_worktrees', { scope: { environment_id: environmentId ?? '', project_id: projectId }, ...(refresh ? {refresh:true} : {}) }, signal)) : await this.request<Snapshot>(`projects/${encodeURIComponent(projectId)}/worktrees${refresh?'/refresh':''}${environmentId ? `?environment_id=${encodeURIComponent(environmentId)}` : ''}`, signal,refresh?'POST':'GET');
    if (environmentId && (snapshot.environment_id !== environmentId || snapshot.project_id !== projectId || snapshot.worktrees.some(tree => tree.environment_id !== environmentId || tree.project_id !== projectId))) throw new ApiError('error');
    return snapshot;
  }
}

