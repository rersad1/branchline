import { lazy, Suspense, useEffect, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { ApiClient, EditRequest } from '../../api';
import { changeDraft, draftKey, draftRevision, openDraft, readExternal, refreshDraft, resolveExternal, saveDraft, subscribeDrafts, type Draft } from './drafts';
const MonacoWorking = lazy(() => import('./MonacoPatch').then(module => ({ default: module.MonacoWorking })));
export function WorkingFile({ api, environment, request, sideBySide, connected, fallback, observation }: { api: ApiClient; environment: string; request: EditRequest; sideBySide: boolean; connected: boolean; fallback?: ReactNode; observation?: string }) {
  const [draft, setDraft] = useState<Draft>(); const [error, setError] = useState('');
  const key = draftKey(environment, request);
  useSyncExternalStore(subscribeDrafts, draftRevision);
  useEffect(() => {
    let active = true;
    void openDraft(api, environment, request).then(value => { if (active) setDraft(value); }, error => { if (active) setError(error.message); });
    return () => { active = false; };
  }, [api, key]);
  useEffect(() => { if (draft && connected) void refreshDraft(draft); }, [draft, connected, observation]);
  if (error) return <><p className="diff-message" role="alert">{error}</p>{fallback}</>;
  if (!draft || draft.key !== key) return <><p className="diff-message">Загрузка полного файла…</p>{fallback}</>;
  return <>
    <div className="file-save-status" role="status"><span>{draft.status === 'saving' ? 'Сохранение…' : draft.status === 'dirty' ? 'Есть изменения' : draft.status === 'saved' ? 'Сохранено' : draft.error}</span>
      {draft.status === 'error' && <button onClick={() => void saveDraft(draft).catch(() => {})}>Повторить</button>}
      {draft.status === 'conflict' && !draft.external && <button onClick={() => void readExternal(draft)}>Сравнить с внешним файлом</button>}
      {draft.external && <><span>Слева — внешний файл, справа — ваш черновик.</span><button onClick={() => resolveExternal(draft, true)}>Сохранить мой вариант</button><button onClick={() => resolveExternal(draft, false)}>Принять внешний файл</button></>}
    </div>
    <Suspense fallback={<p className="diff-message">Loading — Monaco…</p>}><MonacoWorking draft={draft} sideBySide={sideBySide} readOnly={!connected} onChange={(text, changes, endings) => changeDraft(draft, text, changes, endings)} onSave={() => void saveDraft(draft).catch(() => {})}/></Suspense>
  </>;
}
