import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ApiProblem, ExecutionEnvironment, Project, Snapshot, DirectoryPage, DiffRequest, DiffResponse, CreateRequest, CreateResponse, RemoveRequest, RemoveResponse, TerminalRequest, TerminalResponse } from './api';

export const desktopMode = () => isTauri();
export interface RuntimeState { environment: ExecutionEnvironment; state: 'connected' | 'disconnected' | 'error'; error: ApiProblem | null }
export interface Selection { environment_id: string; project_id: string; worktree_id: string | null }
export type TerminalLaunch = {kind:'shell'} | {kind:'harness';installation_id:string};
export interface TerminalLayout { environment_id: string; project_id: string; worktree_id: string; tabs: { id: string; title: string; launch?:TerminalLaunch }[]; active_id: string | null; split_id: string | null; vertical: boolean }
export interface Preferences { light_theme: boolean; notifications: boolean; system_notifications: boolean; close_without_confirmation: boolean; ide: Record<string,string> }
export const defaultPreferences: Preferences = { light_theme: false, notifications: false, system_notifications: false, close_without_confirmation: false, ide: {} };
export interface Layout { preferences?: Preferences; terminals?: TerminalLayout[]; explorer_visible: boolean; explorer_width: number; collapsed: string[] }
export interface Workspace { version: number; environments: { environment: ExecutionEnvironment; registration_id: string }[]; selection: Selection | null; layout: Layout }
interface Commands {
  memory_diagnostics: {args:{action:'status'|'start'|'stop'|'export'};result:import('./features/diagnostics/types').MemoryStatus};
  notify_attention: {args:{target:{environment_id:string;project_id:string;worktree_id:string;instance_id:string;session_id:string;sequence:number}};result:string|null};
  close_application: {args: undefined; result: void};
  harnesses: {args:{scope:{environment_id:string};request:HarnessRequest};result:HarnessResponse};
  agents: {args:{scope:{environment_id:string};request:AgentRequest};result:AgentResponse};
  provider: { args: { scope: { environment_id: string }; request: ProviderRequest }; result: ProviderResponse };
  terminal: { args: { scope: { environment_id: string }; request: TerminalRequest }; result: TerminalResponse };
  remove_worktree: { args: { scope: { environment_id: string }; request: RemoveRequest }; result: RemoveResponse };
  create_worktree: { args: { scope: { environment_id: string }; request: CreateRequest }; result: CreateResponse };
  edit_file: { args: { scope: { environment_id: string }; request: import('./api').EditRequest }; result: import('./api').EditResponse };
  read_diff: { args: { scope: { environment_id: string }; request: DiffRequest }; result: DiffResponse };
  load_workspace: { args: undefined; result: Workspace };
  save_workspace_ui: { args: { selection: Selection | null; layout: Layout }; result: void };
  relink_wsl: { args: { environmentId: string }; result: ExecutionEnvironment };
  remove_project: { args: { scope: { environment_id: string; project_id: string } }; result: void };
  list_wsl_distros: { args: undefined; result: string[] };
  pick_windows_folder: { args: undefined; result: string | null };
  add_project: { args: { scope: { environment_id: string }; rootPath: string }; result: Project };
  browse_directories: { args: { scope: { environment_id: string }; request: { path: string; offset: number } }; result: DirectoryPage };
  runtime_environment: { args: undefined; result: ExecutionEnvironment };
  runtime_states: { args: undefined; result: RuntimeState[] };
  connect_wsl: { args: { distro: string; expectedId: string | null }; result: ExecutionEnvironment };
  list_projects: { args: { scope: { environment_id: string } }; result: Project[] };
  list_worktrees: { args: { scope: { environment_id: string; project_id: string }; refresh?: boolean }; result: Snapshot };
}
export interface Installation {id:string;environment_id:string;name:string;executable:string;version:string|null;protocol:'acp'|'native'|'terminal';compatibility:string;capabilities:string[];manual:boolean;npm?:{node:string;package:string;entry:string;name:string}}
export type HarnessRequest={request_id:string}&({action:'discover'}|{action:'add';executable:string;node?:string});
export interface HarnessResponse{environment_id:string;request_id:string;installations:Installation[]}
export type AgentAction={action:'attach';port:number;instance_id:string}|{action:'list'}|{action:'launch';installation_id:string;instance_id:string;candidate?:boolean}|{action:'stop'|'new'|'history';instance_id:string}|{action:'prompt';instance_id:string;session_id:string;text:string}|{action:'cancel'|'load';instance_id:string;session_id:string}|{action:'mode';instance_id:string;session_id:string;mode_id:string}|{action:'config';instance_id:string;session_id:string;config_id:string;value:string}|{action:'permission';instance_id:string;permission_id:number;option_id:string|null};
export type AgentRequest=AgentAction&{project_id:string;worktree_id:string;request_id:string};
export interface AgentSession{id:string;parent_id:string|null;directory:string|null;assigned:boolean;activity:'Unknown'|'Working'|'Idle'|'Waiting'|'Error';modes:{id:string;name:string}[];current_mode:string|null;config_options?:{id:string;name:string;current_value:string;options:{id:string;name:string}[]}[];commands?:{name:string;description:string}[]}
export interface AgentInstance{id:string;environment_id:string;installation_id:string;project_id:string;worktree_id:string;directory:string;connection:string;stale:boolean;error:string|null;owned:boolean;protocol_version:number|null;capabilities:{load:boolean;list:boolean;image:boolean;audio:boolean;embedded_context:boolean};sessions:AgentSession[];permissions:{id:number;session_id:string;title:string;options:{id:string;name:string}[]}[];events:{sequence:number;session_id:string|null;kind:string;text:string}[];cursor:number}
export interface AgentResponse{environment_id:string;project_id:string;worktree_id:string;request_id:string;instances:AgentInstance[]}
export interface ProviderRequest { project_id: string; worktree_id: string; request_id: string; port: number; cancel: boolean }
export interface ProviderResponse {
  environment_id: string; project_id: string; worktree_id: string; request_id: string; port: number;
  observation: null | { version: string; directory: string; observed_at: string; sse_connected: boolean; sessions: { id: string; status: 'Unknown' | 'Working' | 'Idle' }[] };
}
export async function inspectProvider(environment_id: string, request: ProviderRequest, signal: AbortSignal): Promise<ProviderResponse> {
  const cancel = () => { void command('provider', { scope: { environment_id }, request: { ...request, cancel: true } }).catch(() => {}); };
  if (signal.aborted) throw new DOMException('Cancelled', 'AbortError');
  signal.addEventListener('abort', cancel, { once: true });
  try {
    const value = await command('provider', { scope: { environment_id }, request }, signal);
    if (value.environment_id !== environment_id || value.project_id !== request.project_id || value.worktree_id !== request.worktree_id || value.request_id !== request.request_id || value.port !== request.port || !value.observation) throw new Error('Provider response scope mismatch');
    return value;
  } finally { signal.removeEventListener('abort', cancel); }
}
export function command<K extends keyof Commands>(name: K, args: Commands[K]['args'], signal?: AbortSignal): Promise<Commands[K]['result']> {
  if (signal?.aborted) return Promise.reject(new DOMException('Cancelled', 'AbortError'));
  // Tauri cannot cancel an invoke. Discard cancelled results; Rust independently bounds work.
  return new Promise((resolve, reject) => {
    const abort = () => reject(new DOMException('Cancelled', 'AbortError'));
    signal?.addEventListener('abort', abort, { once: true });
    invoke<Commands[K]['result']>(name, args).then(value => {
      if (!signal?.aborted) resolve(value);
    }, reject).finally(() => signal?.removeEventListener('abort', abort));
  });
}
export const onRuntimeState = (callback: (state: RuntimeState) => void) => listen<RuntimeState>('runtime-state', event => callback(event.payload));
export const onCloseRequested = (callback: () => void) => listen('app-close-requested', callback);
export const onAttentionUnavailable = (callback: (message:string) => void) => listen<string>('attention-unavailable', event=>callback(event.payload));
export const onAttentionOpen = (callback: (target: {environment_id:string;project_id:string;worktree_id:string;instance_id:string;session_id:string;sequence:number}) => void) => listen<{environment_id:string;project_id:string;worktree_id:string;instance_id:string;session_id:string;sequence:number}>('attention-open', event=>callback(event.payload));

