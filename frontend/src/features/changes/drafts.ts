import type { ApiClient, EditRequest, EditResponse } from '../../api';

export type Draft = {
  key: string; api: ApiClient; environment: string; request: EditRequest;
  text: string; raw: string; saved: string; savedRaw: string; original: string; version: string;
  status: 'saved' | 'dirty' | 'saving' | 'error' | 'conflict'; error?: string;
  external?: EditResponse; timer?: ReturnType<typeof setTimeout>; pending?: Promise<void>;
};
const entries = new Map<string, Draft>();
const loading = new Map<string, Promise<Draft>>();
let requestedDraft: Draft | undefined;
export function requestDraft(draft: Draft) { requestedDraft = draft; window.dispatchEvent(new CustomEvent('changes-open-draft')); }
export function takeRequestedDraft(environment: string, worktree: string) {
  if (requestedDraft?.environment !== environment || requestedDraft.request.worktree_id !== worktree) return;
  const draft = requestedDraft; requestedDraft = undefined; return draft;
}
const normalize = (text: string) => text.replace(/\r\n|\r/g, '\n');
const listeners = new Set<() => void>();
let revision = 0;
export const subscribeDrafts = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const draftRevision = () => revision;
function emit() { revision++; listeners.forEach(fn => fn()); }
export const allDrafts = () => [...entries.values()];
export const draftKey = (environment: string, request: Pick<EditRequest, 'project_id' | 'worktree_id' | 'path'>) => JSON.stringify([environment, request.project_id, request.worktree_id, request.path]);
export async function openDraft(api: ApiClient, environment: string, request: EditRequest): Promise<Draft> {
  const key = draftKey(environment, request);
  const current = entries.get(key); if (current) return current;
  const existing = loading.get(key); if (existing) return existing;
  const operation = api.editFile(environment, { ...request, text: null, version: null }).then(result => {
    const draft: Draft = { key, api, environment, request: { ...request, text: null, version: null }, text: normalize(result.text), raw: result.text, savedRaw: result.text, saved: normalize(result.text), original: result.original, version: result.version, status: 'saved' };
    entries.set(key, draft); emit(); return draft;
  }).finally(() => loading.delete(key));
  loading.set(key, operation); return operation;
}
type TextChange = { rangeOffset: number; rangeLength: number; text: string };
function rawOffset(raw: string, offset: number) {
  let i=0, n=0;
  while(i<raw.length && n<offset) { if(raw[i]==='\r' && raw[i+1]==='\n') i++; i++; n++; }
  return i;
}
export function changeDraft(draft: Draft, text: string, changes?: readonly TextChange[], restoreEndings?: readonly string[]) {
  if (text === draft.text && !restoreEndings) return;
  if (restoreEndings) {
    let index = 0; draft.raw = text.replace(/\n/g, () => restoreEndings[index++] ?? '\n');
  } else if (changes) {
    // Monaco offsets are UTF-16 offsets into its LF model. Keep untouched raw bytes'
    // line endings; new line breaks inherit the local line's original ending.
    for (const change of [...changes].sort((a,b)=>b.rangeOffset-a.rangeOffset)) {
      const start=rawOffset(draft.raw,change.rangeOffset), end=rawOffset(draft.raw,change.rangeOffset+change.rangeLength);
      const eol=draft.raw.slice(start).match(/\r\n|\r|\n/)?.[0] ?? draft.raw.slice(0,start).match(/\r\n|\r|\n/g)?.at(-1) ?? '\n';
      draft.raw=draft.raw.slice(0,start)+normalize(change.text).replace(/\n/g,eol)+draft.raw.slice(end);
    }
  } else draft.raw = text.replace(/\n/g, draft.raw.match(/\r\n|\r|\n/)?.[0] ?? '\n');
  draft.text = text; clearTimeout(draft.timer);
  if (draft.status !== 'conflict' && draft.status !== 'error') {
    draft.status = draft.pending ? 'saving' : draft.raw === draft.savedRaw ? 'saved' : 'dirty';
    draft.timer = setTimeout(() => { void saveDraft(draft).catch(() => {}); }, 700);
  }
  emit();
}
export async function saveDraft(draft: Draft): Promise<void> {
  clearTimeout(draft.timer);
  if (draft.pending) { await draft.pending; if (draft.raw !== draft.savedRaw) return saveDraft(draft); return; }
  if (draft.status === 'conflict') throw new Error(draft.error);
  if (draft.raw === draft.savedRaw) { draft.status = 'saved'; draft.error = undefined; emit(); return; }
  const text = draft.text;
  const raw = draft.raw;
  draft.status = 'saving'; draft.error = undefined; emit();
  draft.pending = draft.api.editFile(draft.environment, { ...draft.request, text: raw, version: draft.version }).then(result => {
    // Only the acknowledged snapshot becomes saved. The editor/model is never replaced.
    draft.saved = text; draft.savedRaw = raw; draft.version = result.version; draft.original = result.original;
    draft.status = draft.raw === raw ? 'saved' : 'dirty';
    window.dispatchEvent(new CustomEvent('changes-saved', { detail: draft.key }));
  }).catch(error => {
    draft.status = error.code === 'edit_conflict' ? 'conflict' : 'error';
    draft.error = error.message ?? 'Ошибка сохранения. Черновик сохранён.'; throw error;
  }).finally(() => { draft.pending = undefined; emit(); });
  await draft.pending;
  if (draft.raw !== draft.savedRaw) await saveDraft(draft);
}
export async function readExternal(draft: Draft) {
  try { draft.external = await draft.api.editFile(draft.environment, { ...draft.request, text: null, version: null }); emit(); }
  catch (error) { draft.error = (error as Error).message; emit(); }
}
export async function refreshDraft(draft: Draft) {
  if (draft.pending || draft.status !== 'saved') return;
  const version = draft.version;
  try {
    const result = await draft.api.editFile(draft.environment, { ...draft.request, text: null, version: null });
    if (draft.pending || draft.status !== 'saved' || draft.version !== version) return;
    draft.version = result.version; draft.raw = result.text; draft.savedRaw = result.text; draft.text = normalize(result.text); draft.saved = normalize(result.text);
    draft.original = result.original; draft.error = undefined; emit();
  } catch (error) { if (draft.status === 'saved' && draft.version === version) { draft.error = (error as Error).message; draft.status = 'error'; emit(); } }
}
export function resolveExternal(draft: Draft, keepMine: boolean) {
  if (!draft.external) return;
  const external = draft.external; draft.external = undefined;
  draft.saved = normalize(external.text); draft.savedRaw = external.text; draft.version = external.version;
  draft.original = external.original; draft.error = undefined;
  if (!keepMine) { draft.raw = external.text; draft.text = normalize(external.text); }
  draft.status = draft.raw === draft.savedRaw ? 'saved' : 'dirty'; emit();
  if (keepMine) void saveDraft(draft).catch(() => {});
}
export async function flushDrafts() {
  await Promise.all(allDrafts().map(draft => saveDraft(draft)));
}
export function discardDrafts() {
  for (const draft of entries.values()) {
    if (draft.pending) throw new Error('Сохранение ещё выполняется. Дождитесь завершения.');
    clearTimeout(draft.timer); draft.text = draft.saved; draft.raw = draft.savedRaw; draft.status = 'saved'; draft.error = undefined;
  }
  emit();
}
