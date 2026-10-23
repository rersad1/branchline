import { useEffect, useRef, useState } from 'react';
import { ApiError, type ApiClient, type Project, type CreateResponse } from '../../api';
import type { EnvironmentView } from './Dashboard';
import { Button } from '../../components/ui/button';

export function CreateWorktree({ api, projects, environments, close, created }: {
  api: ApiClient; projects: Project[]; environments: EnvironmentView[]; close: () => void;
  created: (result: CreateResponse) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const alive = useRef(false);
  const sending = useRef(false);
  const [environment, setEnvironment] = useState('');
  const [repository, setRepository] = useState('');
  const [base, setBase] = useState('HEAD');
  const [branch, setBranch] = useState('');
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<CreateResponse | null>(null);
  const [uncertain, setUncertain] = useState(false);
  useEffect(() => { alive.current = true; dialog.current?.showModal(); return () => { alive.current = false; }; }, []);
  const env = environments.find(e => e.environment.id === environment);
  const project = projects.find(p => p.environment_id === environment && p.id === repository);
  const valid = !!project && env?.state === 'connected' && !env.projects_error &&
    !!base.trim() && !!branch.trim() && !/^[\-]|[\x00-\x1f]/.test(base) && !/^[\-]|[\x00-\x20]/.test(branch) &&
    (env.environment.kind === 'windows' ? /^[A-Za-z]:[\\/]/.test(path) : path.startsWith('/'));
  async function submit() {
    if (!valid || sending.current || result || uncertain) return;
    sending.current = true; setBusy(true); setError('');
    try {
      const response = await api.createWorktree(environment, { project_id: repository, base_ref: base, branch, path });
      if (!alive.current) return;
      setResult(response); created(response);
    } catch (e) {
      if (!alive.current) return;
      setError((e as Error).message);
      // A lost response is not evidence that a write failed. Never automatically retry.
      setUncertain(!(e instanceof ApiError && e.code?.startsWith('create_')));
    } finally { sending.current = false; if (alive.current) setBusy(false); }
  }
  return <dialog ref={dialog} className="add-project create-worktree" aria-labelledby="create-worktree-title" onCancel={e => { e.preventDefault(); if (!busy) close(); }}>
    <h2 id="create-worktree-title">Create worktree</h2>
    <p className="muted">Новая ветка и отдельная папка в выбранной среде.</p>
    <form onSubmit={e => { e.preventDefault(); void submit(); }}>
      <fieldset disabled={busy || !!result || uncertain}>
        <label htmlFor="create-environment">Среда worktree</label>
        <select id="create-environment" value={environment} onChange={e => { setEnvironment(e.target.value); setRepository(''); setPath(''); setError(''); }}>
          <option value="">Выберите среду…</option>
          {environments.map(e => <option key={e.environment.id} value={e.environment.id} disabled={e.state !== 'connected'}>{e.environment.kind === 'windows' ? 'Windows' : `WSL · ${e.environment.distro}`} · {e.state}</option>)}
        </select>
        <label htmlFor="create-repository">Repository</label>
        <select id="create-repository" value={repository} onChange={e => { setRepository(e.target.value); setBase(projects.find(p => p.id === e.target.value && p.environment_id === environment)?.base_ref || 'HEAD'); }}>
          <option value="">Выберите repository…</option>
          {projects.filter(p => p.environment_id === environment).map(p => <option key={p.id} value={p.id}>{p.name} · {p.root_path}</option>)}
        </select>
        <label htmlFor="create-base">Base ref</label><input id="create-base" required maxLength={256} value={base} onChange={e => setBase(e.target.value)}/>
        <label htmlFor="create-branch">Новая ветка</label><input id="create-branch" required maxLength={256} placeholder="feat/my-task" value={branch} onChange={e => setBranch(e.target.value)}/>
        <label htmlFor="create-path">Полный путь ({env?.environment.kind === 'wsl' ? `WSL · ${env.environment.distro}` : 'Windows'})</label>
        <input id="create-path" required maxLength={4096} value={path} placeholder={env?.environment.kind === 'wsl' ? '/home/user/worktrees/my-task' : 'C:\\worktrees\\my-task'} onChange={e => setPath(e.target.value)}/>
        <p className="muted">Папка должна отсутствовать, родитель — существовать. Можно вне исходного repo. Rust проверит путь и Git в выбранной среде.</p>
      </fieldset>
      {busy && <p role="status">Создание… Git и Explorer проверяются в выбранной среде.</p>}
      {error && <p role="alert">{error}</p>}
      {uncertain && <p role="alert">Результат не подтверждён. Проверьте Explorer, ветку и путь перед новой попыткой. Автоматического повтора или удаления файлов нет.</p>}
      {result && <section role={result.warning ? 'alert' : 'status'}>
        <p>{result.git_created ? 'Worktree создан Git.' : 'Создание не завершено.'} {result.warning}</p>
        <code>{result.path}</code>
        <p>Ветка: {result.branch_exists === null ? 'не подтверждена' : result.branch_exists ? 'существует' : 'отсутствует'} · Путь: {result.path_exists === null ? 'не подтверждён' : result.path_exists ? 'существует' : 'отсутствует'}</p>
      </section>}
      <div className="dialog-actions"><Button type="button" variant="outline" disabled={busy} onClick={close}>{result || uncertain ? 'Закрыть' : 'Отмена'}</Button>
        {!result && !uncertain && <Button type="submit" disabled={!valid || busy}>Создать worktree</Button>}
      </div>
    </form>
  </dialog>;
}
