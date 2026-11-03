import {useEffect, useRef, useState} from 'react';
import {command, onUpdateStatus, type UpdateStatus} from '../../desktop';
import {AgentDialog} from '../dashboard/AgentDialog';
import {Icon} from '../../components/ui/icon';

const initial:UpdateStatus={current_version:'',phase:'idle',version:null,downloaded:0,total:null,error:null};
export function Updates({ready,install}:{ready:boolean;install:()=>Promise<UpdateStatus>}) {
  const [status,setStatus]=useState(initial),[open,setOpen]=useState(false),[error,setError]=useState(''),[working,setWorking]=useState(false);
  const running=useRef(false),latest=useRef(status),generation=useRef(0);latest.current=status;
  const busy=working||['checking','downloading','installing'].includes(status.phase);
  async function run(action:'check'|'download'|'install') {
    if(running.current||['checking','downloading','installing'].includes(latest.current.phase))return;
    running.current=true;generation.current++;setWorking(true);setError('');
    try { setStatus(action==='install'?await install():await command('updates',{action})); }
    catch(e) { setError(e instanceof Error?e.message:'Не удалось выполнить обновление. Повторите позже.'); }
    finally { running.current=false;setWorking(false); }
  }
  const runRef=useRef(run);runRef.current=run;
  useEffect(()=>{
    if(!ready)return;
    let active=true;
    const revision=generation.current;
    void command('updates',{action:'status'}).then(value=>{if(active&&revision===generation.current)setStatus(value);}).catch(()=>{});
    const subscription=onUpdateStatus(value=>{if(active)setStatus(value);}).catch(()=>()=>{});
    const first=setTimeout(()=>void runRef.current('check'),8000);
    const interval=setInterval(()=>void runRef.current('check'),6*60*60*1000);
    return()=>{active=false;clearTimeout(first);clearInterval(interval);void subscription.then(off=>off());};
  },[ready]);
  const available=!!status.version;
  return <>
    <button className="icon-button update-control" aria-label="Обновления Branchline" title={available?`Доступна версия ${status.version}`:'Обновления Branchline'} disabled={!ready} onClick={()=>setOpen(true)}><Icon name="refresh"/>{available&&<span className="update-dot"/>}</button>
    {open&&<AgentDialog title="Обновления Branchline" close={()=>setOpen(false)}>
      {status.current_version&&<p>Установлена версия {status.current_version}.</p>}
      <p role="status">{status.phase==='checking'?'Проверяем обновления…':status.phase==='downloading'?'Скачиваем обновление…':status.phase==='installing'?'Сохраняем работу и устанавливаем обновление…':status.phase==='ready'?`Обновление ${status.version} скачано и проверено.`:status.phase==='current'?'Установлена последняя версия.':available?`Доступна версия ${status.version}.`:'Новые версии проверяются автоматически.'}</p>
      {status.phase==='downloading'&&<progress aria-label="Скачивание обновления" value={status.total?status.downloaded:undefined} max={status.total??undefined}/>}
      {(error||status.error)&&<p role="alert">{error||status.error}</p>}
      {status.phase==='ready'&&<p>Файлы Changes будут сохранены, Branchline перезапустится. Работающие терминалы и агенты будут завершены; после обновления их можно запустить снова.</p>}
      <div className="dialog-actions">
        <button disabled={busy||status.phase==='ready'} onClick={()=>void run('check')}>Проверить обновления</button>
        {available&&status.phase!=='ready'&&<button disabled={busy} onClick={()=>void run('download')}>Скачать обновление</button>}
        {status.phase==='ready'&&<button disabled={busy} onClick={()=>void run('install')}>Сохранить и перезапустить</button>}
      </div>
    </AgentDialog>}
  </>;
}
