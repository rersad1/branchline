import { useEffect, useRef, useState } from 'react';
import { ApiError, type ApiClient, type Worktree, type RemoveResponse } from '../../api';
import { Button } from '../../components/ui/button';
export function RemoveWorktree({api,tree,close,removed}:{api:ApiClient;tree:Worktree;close:()=>void;removed:(r:RemoveResponse)=>void}) {
  const dialog=useRef<HTMLDialogElement>(null),alive=useRef(false),sending=useRef(false);
  const missing=!tree.is_available&&tree.is_prunable;
  const [path,setPath]=useState(''),[checked,setChecked]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[result,setResult]=useState<RemoveResponse|null>(null),[unknown,setUnknown]=useState(false);
  useEffect(()=>{alive.current=true;dialog.current?.showModal();return()=>{alive.current=false;};},[]);
  async function submit() {
    if(sending.current||path!==tree.path||!checked||result||unknown)return;
    sending.current=true;setBusy(true);setError('');
    try{const r=await api.removeWorktree(tree.environment_id,{project_id:tree.project_id,worktree_id:tree.id,confirm_path:path,external_sessions_checked:checked,...(missing?{missing_only:true}:{})});if(alive.current){setResult(r);removed(r);}}
    catch(e){if(alive.current){setError((e as Error).message);setUnknown(!(e instanceof ApiError&&e.code?.startsWith('remove_')));}}
    finally{sending.current=false;if(alive.current)setBusy(false);}
  }
  return <dialog ref={dialog} className="add-project create-worktree" aria-labelledby="remove-worktree-title" onCancel={e=>{e.preventDefault();if(!busy)close();}}>
    <h2 id="remove-worktree-title">{missing?'Удалить недоступный worktree':'Remove worktree'}</h2>
    <p>{missing?'Папка не найдена. Git удалит оставшуюся запись этого worktree. Ветка сохранится. Если папка снова доступна, обновите список.':'Git удалит чистый linked worktree и его папку. Ветка сохранится.'}</p><code>{tree.path}</code>
    <p className="muted">Dirty, untracked, conflicts, locked, Primary и активные терминалы приложения блокируют удаление. Внешние сессии автоматически не считаются idle.</p>
    <form onSubmit={e=>{e.preventDefault();void submit();}}><fieldset disabled={busy||!!result||unknown}>
      <label htmlFor="remove-path">Подтвердите полный путь</label><input id="remove-path" value={path} onChange={e=>setPath(e.target.value)} autoComplete="off"/>
      <label className="external-session-check"><input type="checkbox" checked={checked} onChange={e=>setChecked(e.target.checked)}/>Я вручную проверил и закрыл внешние терминалы/сессии этого worktree</label>
    </fieldset>
      {busy&&<p role="status">Повторная проверка и Git remove…</p>}{error&&<p role="alert">{error}</p>}
      {unknown&&<p role="alert">Результат не подтверждён. Проверьте Explorer и путь перед новой попыткой; автоматического повтора нет.</p>}
      {result&&<p role={result.warning?'alert':'status'}>{result.git_removed?'Worktree удалён Git.':'Git не подтвердил удаление.'} {result.warning} Папка: {result.path_exists===null?'не подтверждена':result.path_exists?'существует':'отсутствует'}.</p>}
      <div className="dialog-actions"><Button type="button" variant="outline" disabled={busy} onClick={close}>{result||unknown?'Закрыть':'Отмена'}</Button>{!result&&!unknown&&<Button type="submit" disabled={busy||path!==tree.path||!checked}>Удалить worktree</Button>}</div>
    </form>
  </dialog>;
}
