// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { ApiClient, ApiError, type EditRequest, type EditResponse } from '../src/api';
import { changeDraft, openDraft, saveDraft, readExternal, resolveExternal, discardDrafts, refreshDraft } from '../src/features/changes/drafts';
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); discardDrafts(); });
let id=0;
const request=(): EditRequest => ({project_id:'project',worktree_id:`tree-${id++}`,path:'каталог с пробелом/file.txt',text:null,version:null});
const reply=(r:EditRequest,text='initial\n',version='v1'): EditResponse => ({...r,environment_id:'win',text,original:'index\n',version});
it('debounces the full draft and preserves newer input while serialized saves finish', async () => {
  const api=new ApiClient('unused'), r=request();let finish!: (r:EditResponse)=>void;
  const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r));
  const draft=await openDraft(api,'win',r);vi.useFakeTimers();
  edit.mockImplementationOnce(()=>new Promise(resolve=>{finish=resolve;}));
  changeDraft(draft,'first\n');await vi.advanceTimersByTimeAsync(699);expect(edit).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);expect(edit).toHaveBeenCalledTimes(2);
  changeDraft(draft,'second\n');expect(draft.status).toBe('saving');
  edit.mockResolvedValueOnce(reply(r,'second\n','v3'));finish(reply(r,'first\n','v2'));
  await vi.advanceTimersByTimeAsync(0);
  expect(edit.mock.calls[2][1]).toMatchObject({text:'second\n',version:'v2',worktree_id:r.worktree_id});
  expect(draft.text).toBe('second\n');expect(draft.status).toBe('saved');
});
it('retains independent drafts across environments/context changes and keeps write errors retryable',async()=>{
  const api=new ApiClient('unused'),r=request();const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r));
  const win=await openDraft(api,'win',r);edit.mockResolvedValue({...reply(r),environment_id:'ubuntu'});
  const ubuntu=await openDraft(api,'ubuntu',r);changeDraft(win,'Windows only');changeDraft(ubuntu,'Ubuntu only');
  edit.mockRejectedValueOnce(new Error('disk full'));await expect(saveDraft(win)).rejects.toThrow('disk full');
  expect(win.text).toBe('Windows only');expect(win.status).toBe('error');
  expect(await openDraft(api,'win',r)).toBe(win);edit.mockResolvedValue(reply(r,'Windows only','v2'));await saveDraft(win);
  expect(edit).toHaveBeenLastCalledWith('win',expect.objectContaining({text:'Windows only'}));expect(ubuntu.text).toBe('Ubuntu only');
});
it('never auto overwrites an external conflict and requires explicit resolution against a fresh version',async()=>{
  const api=new ApiClient('unused'),r=request();const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r));
  const draft=await openDraft(api,'win',r);changeDraft(draft,'mine');edit.mockRejectedValueOnce(new ApiError('error',409,'edit_conflict'));
  await expect(saveDraft(draft)).rejects.toThrow();changeDraft(draft,'mine newer');expect(draft.status).toBe('conflict');
  await expect(saveDraft(draft)).rejects.toThrow();expect(edit).toHaveBeenCalledTimes(2);
  edit.mockResolvedValueOnce(reply(r,'external','v2'));await readExternal(draft);expect(draft.text).toBe('mine newer');
  edit.mockResolvedValueOnce(reply(r,'mine newer','v3'));resolveExternal(draft,true);await draft.pending;
  expect(edit).toHaveBeenLastCalledWith('win',expect.objectContaining({version:'v2',text:'mine newer'}));expect(draft.status).toBe('saved');
});
it('preserves mixed CRLF/LF/CR endings through Monaco UTF16 offsets and local newlines',async()=>{
  const api=new ApiClient('unused'),r=request();const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r,'😀a\r\nb\nc\rd','v1'));
  const draft=await openDraft(api,'win',r);expect(draft.text).toBe('😀a\nb\nc\nd');
  changeDraft(draft,'😀a\nBee\nc\nd',[{rangeOffset:4,rangeLength:1,text:'Bee'}]);
  changeDraft(draft,'😀a\nBee\nc\nnew\nd',[{rangeOffset:10,rangeLength:0,text:'new\n'}]);
  expect(draft.raw).toBe('😀a\r\nBee\nc\rnew\rd');
  edit.mockResolvedValue(reply(r,draft.raw,'v2'));await saveDraft(draft);expect(edit).toHaveBeenLastCalledWith('win',expect.objectContaining({text:'😀a\r\nBee\nc\rnew\rd'}));
});
it('refreshes clean drafts while ignoring a read started before newer typing',async()=>{
  const api=new ApiClient('unused'),r=request();const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r));const draft=await openDraft(api,'win',r);
  edit.mockResolvedValueOnce(reply(r,'external\r\n','v2'));await refreshDraft(draft);expect(draft.text).toBe('external\n');
  let finish!: (r:EditResponse)=>void;edit.mockImplementationOnce(()=>new Promise(resolve=>{finish=resolve;}));
  const pending=refreshDraft(draft);changeDraft(draft,'my newer draft');finish(reply(r,'obsolete','v3'));await pending;
  expect(draft.text).toBe('my newer draft');expect(draft.version).toBe('v2');
});
it('restores exact EOL provenance on undo after an intervening autosave',async()=>{
  const api=new ApiClient('unused'),r=request();const edit=vi.spyOn(api,'editFile').mockResolvedValue(reply(r,'a\r\nb','v1'));
  const draft=await openDraft(api,'win',r);const endings=draft.raw.match(/\r\n|\r|\n/g)!;
  changeDraft(draft,'ab',[{rangeOffset:1,rangeLength:1,text:''}]);edit.mockResolvedValueOnce(reply(r,'ab','v2'));await saveDraft(draft);
  changeDraft(draft,'a\nb',[{rangeOffset:1,rangeLength:0,text:'\n'}],endings);expect(draft.raw).toBe('a\r\nb');
  edit.mockResolvedValueOnce(reply(r,'a\r\nb','v3'));await saveDraft(draft);expect(edit).toHaveBeenLastCalledWith('win',expect.objectContaining({text:'a\r\nb',version:'v2'}));
});
