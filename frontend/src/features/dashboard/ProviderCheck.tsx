import { useEffect, useRef, useState } from 'react';
import { inspectProvider, type ProviderResponse } from '../../desktop';
import type { Worktree } from '../../api';

/** Explicit one-shot observation. No saved endpoint, monitoring or managed sessions. */
export function ProviderCheck({ tree, connected }: { tree: Worktree; connected: boolean }) {
  const [port, setPort] = useState('4096');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<ProviderResponse | null>(null);
  const active = useRef<AbortController | null>(null);
  useEffect(() => () => { active.current?.abort(); active.current = null; }, []);
  useEffect(() => { if (!connected) { active.current?.abort(); active.current = null; setBusy(false); setResult(null); } }, [connected]);
  async function inspect() {
    active.current?.abort();
    const controller = new AbortController(); active.current = controller;
    setBusy(true); setError(''); setResult(null);
    try {
      const value = await inspectProvider(tree.environment_id, { project_id: tree.project_id, worktree_id: tree.id, request_id: crypto.randomUUID(), port: Number(port), cancel: false }, controller.signal);
      if (active.current === controller && !controller.signal.aborted) setResult(value);
    } catch (e) {
      if (active.current === controller && !controller.signal.aborted) setError((e as Error).message);
    } finally { if (active.current === controller) { active.current = null; setBusy(false); } }
  }
  return <section aria-label="OpenCode adapter check">
    <h3>OpenCode 1.19.0</h3>
    <p className="muted">Разовая проверка в выбранной среде. Наблюдение не обновляется автоматически; текущий статус агента — Unknown.</p>
    <label>Local port <input aria-label="OpenCode local port" type="number" min="1" max="65535" value={port} disabled={busy} onChange={e => { setPort(e.target.value); setResult(null); }}/></label>{' '}
    <button className="small-action" disabled={busy || !connected || !tree.is_available || !/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65535} onClick={() => void inspect()}>Проверить OpenCode</button>{' '}
    {busy && <button className="small-action" onClick={() => { active.current?.abort(); active.current = null; setBusy(false); }}>Отменить</button>}
    {busy && <p role="status">Проверка контракта…</p>}
    {error && <p role="alert">{error}</p>}
    {result?.observation && <p role="status">Snapshot {result.observation.observed_at}: API {result.observation.version} · directory verified · SSE connected · {result.observation.sessions.length} sessions. Соединение проверки закрыто.</p>}
  </section>;
}
