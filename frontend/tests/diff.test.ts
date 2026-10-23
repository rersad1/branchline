import { afterEach, expect, it, vi } from 'vitest';
const invoke=vi.hoisted(()=>vi.fn());
const platform=vi.hoisted(()=>({desktop:true}));
vi.mock('@tauri-apps/api/core',()=>({isTauri:()=>platform.desktop,invoke}));
import { ApiClient, type DiffRequest } from '../src/api';
afterEach(()=>{invoke.mockReset();platform.desktop=true;vi.restoreAllMocks();});
const request:DiffRequest={project_id:'project',worktree_id:'tree',mode:'unstaged',base_ref:null,file_id:null,context_lines:3};
const response={environment_id:'env',...request,state:'ready',files:[],selected_file:null};
it('uses typed scoped diff IPC and sends no token or filesystem path',async()=>{
 const fetch=vi.spyOn(globalThis,'fetch');invoke.mockResolvedValue(response);await new ApiClient('never-send').diff('env',request);
 expect(invoke).toHaveBeenCalledWith('read_diff',{scope:{environment_id:'env'},request});expect(fetch).not.toHaveBeenCalled();expect(JSON.stringify(invoke.mock.calls)).not.toContain('never-send');
});
it('rejects foreign worktree/mode/file/context and preserves explicit no-base fallback',async()=>{
 const api=new ApiClient('');for(const mismatch of [{environment_id:'other'},{worktree_id:'other'},{mode:'staged'},{context_lines:100},{selected_file:{file_id:'other',state:'text',patch:'foreign'}}]){invoke.mockResolvedValueOnce({...response,...mismatch});await expect(api.diff('env',request)).rejects.toMatchObject({kind:'error'});}
 invoke.mockResolvedValueOnce({...response,mode:'branch',base_ref:'missing',state:'no_base'});expect((await api.diff('env',{...request,mode:'branch',base_ref:'missing',file_id:'old'})).state).toBe('no_base');
});
it('discards a cancelled late diff and keeps sanitized domain failure visible',async()=>{
 let finish!:(value:unknown)=>void;invoke.mockReturnValueOnce(new Promise(r=>{finish=r;}));const controller=new AbortController();const result=new ApiClient('').diff('env',request,controller.signal);controller.abort();await expect(result).rejects.toMatchObject({name:'AbortError'});finish(response);
 invoke.mockRejectedValueOnce({code:'diff_changed',message:'Refresh required'});await expect(new ApiClient('').diff('env',request)).rejects.toMatchObject({message:'Refresh required'});
});
it('maps browser transport and malformed responses to API errors and preserves cancellation',async()=>{
 platform.desktop=false;const fetch=vi.spyOn(globalThis,'fetch');const api=new ApiClient('token');
 fetch.mockRejectedValueOnce(new TypeError('network detail'));await expect(api.diff('env',request)).rejects.toMatchObject({kind:'disconnected'});
 fetch.mockResolvedValueOnce({ok:true,json:async()=>{throw Error('invalid JSON');}} as unknown as Response);await expect(api.diff('env',request)).rejects.toMatchObject({kind:'error'});
 const controller=new AbortController();controller.abort();fetch.mockRejectedValueOnce(new DOMException('Cancelled','AbortError'));await expect(api.diff('env',request,controller.signal)).rejects.toMatchObject({name:'AbortError'});
});
