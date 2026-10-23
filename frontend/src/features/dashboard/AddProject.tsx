import { useEffect, useRef, useState } from 'react';
import type { ApiClient, DirectoryPage, ExecutionEnvironment } from '../../api';
import { Button } from '../../components/ui/button';

export function AddProject({ api, windows, close, added }: { api: ApiClient; windows: ExecutionEnvironment; close: () => void; added: (env: ExecutionEnvironment) => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const alive = useRef(true);
  const [distros, setDistros] = useState<string[]>([]);
  const [target, setTarget] = useState('windows');
  const [env, setEnv] = useState(windows);
  const [path, setPath] = useState('');
  const [page, setPage] = useState<DirectoryPage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [distroError, setDistroError] = useState('');
  const [cancelled, setCancelled] = useState(false);
  useEffect(() => {
    alive.current = true; dialog.current?.showModal();
    void api.distros().then(setDistros).catch(e => { if (alive.current) setDistroError((e as Error).message); });
    return () => { alive.current = false; };
  }, [api]);
  async function run(operation: () => Promise<void>) {
    setBusy(true); setError(''); setCancelled(false);
    try { await operation(); } catch (e) { if (alive.current) setError((e as Error).message); }
    finally { if (alive.current) setBusy(false); }
  }
  async function choose(value: string) {
    setTarget(value); setPath(''); setPage(null);
    if (value === 'windows') { setEnv(windows); setError(''); return; }
    await run(async () => {
      const next = await api.connectWsl(value.slice(4));
      if (!alive.current) return;
      setEnv(next); setPath('/');
      const result = await api.directories(next.id, '/');
      if (alive.current) setPage(result);
    });
  }
  async function browse(value: string, offset = 0) {
    await run(async () => {
      const result = await api.directories(env.id, value, offset);
      if (alive.current) { setPage(result); setPath(result.path); }
    });
  }
  const validEnvironment = target === 'windows' ? env.kind === 'windows' : env.distro === target.slice(4);
  return <dialog ref={dialog} className="add-project" onCancel={e => { e.preventDefault(); if (!busy) close(); }} aria-labelledby="add-project-title">
    <h2 id="add-project-title">Добавить проект</h2>
    <p className="muted">Выберите среду, которая будет выполнять Git для этого repository.</p>
    <fieldset disabled={busy}>
      <label htmlFor="project-environment">Среда проекта</label>
      <select id="project-environment" value={target} onChange={e => { void choose(e.target.value); }}>
        <option value="windows">Windows</option>
        {distros.map(d => <option key={d} value={`wsl:${d}`}>WSL · {d}</option>)}
      </select>
      {distroError && <p role="status">WSL список недоступен: {distroError}</p>}
      {target === 'windows' ? <>
        <Button variant="outline" onClick={() => { void run(async () => {
          const selected = await api.pickWindowsFolder();
          if (alive.current) { if (selected !== null) setPath(selected); else setCancelled(true); }
        }); }}>Выбрать папку Windows…</Button>
        <p className="selected-path"><code>{path || 'Папка не выбрана'}</code></p>
      </> : <>
        <label htmlFor="linux-path">Linux path</label>
        <div className="path-controls"><input id="linux-path" value={path} maxLength={4096} placeholder="/home/user/project" onChange={e => setPath(e.target.value)}/>
          <Button variant="outline" disabled={!validEnvironment || !path.startsWith('/')} onClick={() => { void browse(path); }}>Открыть</Button></div>
        {page && <section aria-label="Каталоги Linux" className="directory-browser">
          <div className="directory-toolbar"><code>{page.path}</code><Button variant="outline" disabled={!page.parent} onClick={() => { if (page.parent) void browse(page.parent); }}>Вверх</Button></div>
          <ul>{page.entries.map(entry => <li key={entry.path}><button onClick={() => { void browse(entry.path); }}>{entry.name}{entry.symlink ? ' ↗ (symlink)' : ''}</button></li>)}</ul>
          {!page.entries.length && <p className="muted">Нет доступных подкаталогов.</p>}
          <div className="directory-toolbar"><Button variant="outline" disabled={page.next_offset === null} onClick={() => { if (page.next_offset !== null) void browse(page.path, page.next_offset); }}>Следующие 50</Button><Button variant="outline" onClick={() => { void browse(page.path); }}>Первая страница</Button></div>
        </section>}
      </>}
    </fieldset>
    {busy && <p role="status">Выполняется запрос…</p>}
    {cancelled && <p role="status">Выбор папки отменён.</p>}
    {error && <p role="alert">{error}</p>}
    <div className="dialog-actions"><Button variant="outline" disabled={busy} onClick={close}>Отмена</Button><Button disabled={busy || !path || !validEnvironment} onClick={() => { void run(async () => {
      await api.addProject(env.id, path);
      if (alive.current) added(env);
    }); }}>Добавить</Button></div>
  </dialog>;
}
