import { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { ApiClient, TerminalAction, TerminalSession, Worktree } from '../../api';
import type { TerminalLayout, Installation } from '../../desktop';
import { Icon } from '../../components/ui/icon';
import { attachTerminalInteraction } from './interaction';
import { NewTabMenu } from './NewTabMenu';
import { shellDirectory, terminalTheme } from './shellIntegration';
import { moveTab, numberTabs } from './tabs';

export function TerminalPane({api,tree,session,connected,visible=true}:{api:ApiClient;tree:Worktree;session:TerminalSession;connected:boolean;visible?:boolean}) {
  const element=useRef<HTMLDivElement>(null), live=useRef(connected),available=useRef(connected);
  live.current=connected&&session.state==='running';available.current=connected;
  const [error,setError]=useState('');
  const [directory,setDirectory]=useState<string|null>(null);
  const shell=session.shell==='Windows PowerShell'||session.shell==='Bash';
  // Start a consumer only after its pane has been shown, then retain its VT screen
  // and read cursor while hidden. Replaying a PTY log at a new size corrupts TUIs.
  const [started,setStarted]=useState(visible);
  useEffect(()=>{if(visible)setStarted(true);},[visible]);
  useEffect(()=>{
    if(!started||!element.current)return;
    let alive=true,cursor=0,input='',sending=false,activeUntil=0,wakeRead=()=>{};
    const opencode=session.shell.toLowerCase().replace(/\.exe$/, '')==='opencode';
    const terminal=new Terminal({scrollback:2000,scrollSensitivity:1,fastScrollSensitivity:1,fontFamily:'Cascadia Code, Consolas, monospace',fontSize:13,theme:terminalTheme(document.documentElement.dataset.theme==='light'),cursorStyle:'bar',allowProposedApi:false});
    const fit=new FitAddon();terminal.loadAddon(fit);terminal.open(element.current);
    const themeObserver=new MutationObserver(()=>{terminal.options.theme=terminalTheme(document.documentElement.dataset.theme==='light');});
    themeObserver.observe(document.documentElement,{attributes:true,attributeFilter:['data-theme']});
    let nativePaste=false;
    const integration=shell?terminal.parser.registerOscHandler(633,data=>{if(/^P;NativePaste=[01]$/.test(data)&&session.shell==='Windows PowerShell'){nativePaste=data.endsWith('1');return true;}const path=shellDirectory(data);if(path!==undefined&&alive)setDirectory(path);return path!==undefined;}):undefined;
    const call=(action:TerminalAction)=>api.terminal(tree.environment_id,{project_id:tree.project_id,worktree_id:tree.id,...action});
    let rows=0,cols=0,resizing=false,pendingSize:{rows:number;cols:number}|undefined;
    // Keep the last measured size authoritative even when IPC replies are delayed.
    async function flushResize(){
      if(resizing)return;resizing=true;
      try{while(alive&&live.current&&pendingSize){const next=pendingSize;pendingSize=undefined;try{await call({action:'resize',session_id:session.id,...next});}catch(e){if(alive)setError((e as Error).message);}}}
      finally{resizing=false;}
    }
    const resize=()=>{if(!alive||!element.current?.clientWidth||!element.current?.clientHeight)return;fit.fit();terminal.refresh(0,terminal.rows-1);const r=Math.max(2,Math.min(300,terminal.rows)),c=Math.max(2,Math.min(500,terminal.cols));if(r===rows&&c===cols)return;rows=r;cols=c;if(live.current){pendingSize={rows:r,cols:c};void flushResize();}};
    const observer=new ResizeObserver(resize);observer.observe(element.current);resize();
    let focusedBeforeBlur=false,repairFrame=0;
    const host=element.current;
    const repair=()=>{cancelAnimationFrame(repairFrame);repairFrame=requestAnimationFrame(()=>{if(!alive||!host.clientWidth||!host.clientHeight)return;resize();if(opencode)terminal.scrollToBottom();if(focusedBeforeBlur)terminal.focus();});};
    const blur=()=>{focusedBeforeBlur=host.contains(document.activeElement);};
    const visible=()=>{if(!document.hidden)repair();};
    window.addEventListener('blur',blur);window.addEventListener('focus',repair);document.addEventListener('visibilitychange',visible);
    void document.fonts?.ready.then(()=>{if(alive)repair();});
    const send=(data:string)=>{
      if(!live.current)return;
      if(new TextEncoder().encode(input+data).length>8192){setError('Очередь ввода заполнена; повторите ввод после обработки.');return;}
      input+=data;activeUntil=performance.now()+800;wakeRead();
      if(sending)return;sending=true;
      void(async()=>{try{while(alive&&input){const data=input;input='';await call({action:'input',session_id:session.id,data});if(alive)wakeRead();}}catch(e){if(alive)setError((e as Error).message);}finally{sending=false;}})();
    };
    const subscription=terminal.onData(send);
    const detachInteraction=attachTerminalInteraction(terminal,opencode,send,setError,shell?{nativePaste:()=>nativePaste,host}:undefined);
    const pause=(ms:number)=>new Promise<void>(resolve=>{const timer=setTimeout(done,ms);function done(){clearTimeout(timer);wakeRead=()=>{};resolve();}wakeRead=done;});
    void(async()=>{while(alive){
      if(!available.current){await pause(200);continue;}
      let hadOutput=false;
      try{const response=await call({action:'read',session_id:session.id,cursor});if(!alive)return;
        if(response.lost){nativePaste=false;setDirectory(null);terminal.write('\r\n[Часть output пропущена: bounded buffer]\r\n');}
        if(response.data.length){hadOutput=true;activeUntil=performance.now()+400;await new Promise<void>(r=>terminal.write(new Uint8Array(response.data),r));}
        if(!alive)return;cursor=response.cursor;setError('');
      }catch(e){if(alive)setError((e as Error).message);}
      await pause(hadOutput?0:performance.now()<activeUntil?8:80);
    }})();
    return()=>{alive=false;wakeRead();cancelAnimationFrame(repairFrame);observer.disconnect();themeObserver.disconnect();integration?.dispose();subscription.dispose();detachInteraction();window.removeEventListener('blur',blur);window.removeEventListener('focus',repair);document.removeEventListener('visibilitychange',visible);terminal.dispose();};
  },[api,tree.environment_id,tree.project_id,tree.id,session.id,started]);
  return <section className="terminal-pane" aria-label={`Terminal ${session.id}`}><div className="terminal-meta"><span>{session.shell} · {connected?session.state:'Disconnected'}{session.exit_code!==null?` · exit ${session.exit_code}`:''}</span><code className="terminal-directory" title={`Стартовая: ${session.cwd}\nТекущая: ${directory??'неизвестна'}${!connected?' (нет связи)':''}`} aria-label="Текущая директория">{directory??'Директория неизвестна'}{!connected&&directory?' · нет связи':''}</code><button className="terminal-copy-path" aria-label="Копировать текущую директорию" title="Копировать полный путь" disabled={!directory} onClick={()=>{if(directory)void navigator.clipboard.writeText(directory).catch(()=>setError('Не удалось скопировать путь.'));}}><Icon name="copy" size={12}/></button>{shell&&<span title="Tab / Shift+Tab — дополнение · ↑/↓ и Ctrl+R — история · Ctrl+C — прервать · Ctrl+D — EOF. branchline_prompt compact|user — оформление; branchline_integration_off — отключить до закрытия. BRANCHLINE_SHELL_INTEGRATION=0 — отключить при запуске приложения." className="terminal-help">Shell ⓘ</span>}</div>{(error||session.error)&&<p role="alert">{error||session.error}</p>}<div ref={element} className="terminal-emulator" title={`Ctrl+Shift+C — копировать выделение · Ctrl+Shift+V / Ctrl+V — вставить${session.shell.toLowerCase()==='opencode'?' · Ctrl+C без выделения — прервать запрос':''}`}/></section>;
}
export function TerminalWorkspace({api,tree,connected,layout,save,harness,compact=false,launching=false,installations,onSettings,onActivate}:{api:ApiClient;tree:Worktree;connected:boolean;layout:TerminalLayout|undefined;save:(value:TerminalLayout)=>void;harness?:Installation;compact?:boolean;launching?:boolean;installations?:Installation[];onSettings?:()=>void;onActivate?:(installation:Installation|null)=>void}) {
  const fallback:TerminalLayout={environment_id:tree.environment_id,project_id:tree.project_id,worktree_id:tree.id,tabs:[],active_id:null,split_id:null,vertical:false};
  const raw=layout??fallback;
  const value={...raw,tabs:numberTabs(raw.tabs)};
  const current=useRef(value);current.current=value;
  const tabStrip=useRef<HTMLDivElement>(null);
  const dragged=useRef<string|null>(null);
  useEffect(()=>{tabStrip.current?.querySelector<HTMLElement>('[aria-selected=true]')?.scrollIntoView?.({block:'nearest',inline:'nearest'});},[value.active_id]);
  const alive=useRef(false),busyRef=useRef(false),revision=useRef(0);
  const [sessions,setSessions]=useState<TerminalSession[]>([]),[busy,setBusy]=useState(false),[error,setError]=useState(''),[loading,setLoading]=useState(true),[confirmed,setConfirmed]=useState(false);
  const call=(action:TerminalAction)=>api.terminal(tree.environment_id,{project_id:tree.project_id,worktree_id:tree.id,...action});
  useEffect(()=>{alive.current=true;setLoading(true);setConfirmed(false);let enabled=true;
    void(async()=>{while(enabled){try{if(connected&&!busyRef.current){const version=revision.current;const r=await call({action:'list'});if(!enabled)return;if(version===revision.current&&!busyRef.current){setSessions(r.sessions);setLoading(false);setConfirmed(true);}}}
      catch(e){if(enabled){setError((e as Error).message);setLoading(false);setConfirmed(false);}}await new Promise(r=>setTimeout(r,1000));}})();
    return()=>{enabled=false;alive.current=false;};
  },[api,tree.environment_id,tree.project_id,tree.id,connected]);
  const unified=installations!==undefined;
  async function open(split=false,replacing?:string,target:Installation|null=harness??null) {
    if(busyRef.current||launching||!confirmed||!connected||!tree.is_available||(!replacing&&current.current.tabs.length>=8)||(!unified&&compact&&!harness))return;
    busyRef.current=true;revision.current++;setBusy(true);setError('');
    const id=crypto.randomUUID();
    // Persist intent before invoking. Lost replies can be reattached by the same session ID.
    const old=current.current;
    const name=target?(target.name==='opencode'?'OpenCode':target.name):'Терминал';
    const tab={id,title:`${name} ${replacing?Math.max(1,old.tabs.findIndex(t=>t.id===replacing)+1):old.tabs.length+1}`,launch:target?{kind:'harness' as const,installation_id:target.id}:{kind:'shell' as const}};
    const tabs=replacing?old.tabs.map(t=>t.id===replacing?tab:t):[...old.tabs,tab];
    const next={...old,tabs,active_id:split&&old.active_id?old.active_id:id,split_id:split?id:old.split_id===replacing?null:old.split_id};save(next);
    if(!split)onActivate?.(target);
    try{const r=await call(target?{action:'open_harness',request_id:id,rows:24,cols:80,installation_id:target.id}:{action:'open',request_id:id,rows:24,cols:80});if(alive.current)setSessions(r.sessions);}
    catch(e){if(alive.current)setError(`${(e as Error).message} Проверьте список сессий; автоматического повтора нет.`);}
    finally{busyRef.current=false;if(alive.current){setBusy(false);setLoading(false);}}
  }
  async function close(id:string) {
    if(busyRef.current||launching||!confirmed)return;busyRef.current=true;revision.current++;setBusy(true);setError('');
    try{if(sessions.some(s=>s.id===id))await call({action:'close',session_id:id});if(!alive.current)return;
      const old=current.current,tabs=numberTabs(old.tabs.filter(t=>t.id!==id)),next=tabs[Math.max(0,old.tabs.findIndex(t=>t.id===id)-1)];save({...old,tabs,active_id:old.active_id===id?next?.id??null:old.active_id,split_id:old.split_id===id||old.active_id===id?null:old.split_id});setSessions(s=>s.filter(s=>s.id!==id));
    }catch(e){if(alive.current)setError((e as Error).message);}finally{busyRef.current=false;if(alive.current)setBusy(false);}
  }
  const unlisted=sessions.filter(s=>!value.tabs.some(t=>t.id===s.id));
  const disabled=busy||launching||loading||!confirmed||!connected||!tree.is_available;
  const targetFor=(id:string|null)=>{const tab=value.tabs.find(t=>t.id===id);return tab?.launch?.kind==='shell'?null:tab?.launch?.kind==='harness'?installations?.find(i=>i.id===(tab.launch?.kind==='harness'?tab.launch.installation_id:'')):undefined;};
  function pane(id:string,visible:boolean){const session=sessions.find(s=>s.id===id);return session?<TerminalPane key={id} api={api} tree={tree} session={session} connected={connected} visible={visible}/>:<section className="terminal-stopped" key={id}><p>Остановлен / сессия не подтверждена после restart.</p><p className="muted">Layout восстановлен. Процесс автоматически не запускается.</p>{unified&&targetFor(id)===undefined?<NewTabMenu installations={installations!} disabled={disabled} label="Запустить заново…" create={target=>void open(false,id,target)} settings={onSettings}/>:<button disabled={disabled||(!unified&&compact&&!harness)} onClick={()=>{void open(false,id,unified?targetFor(id)!:harness??null);}}>Запустить заново</button>}</section>;}
  const toolbar = <div className="terminal-toolbar">{!compact&&!unified&&<button data-command="new-terminal" disabled={disabled||value.tabs.length>=8} onClick={()=>{void open();}}><Icon name="plus" size={14}/>Новый терминал</button>}{unified&&targetFor(value.active_id)===undefined?<NewTabMenu installations={installations!} disabled={disabled||!value.active_id||value.tabs.length>=8} label="Разделить рабочую область" icon="split" create={target=>void open(true,undefined,target)} settings={onSettings}/>:<button className="icon-button" title="Разделить рабочую область" aria-label="Split" data-command="split" disabled={disabled||!value.active_id||value.tabs.length>=8||(!unified&&compact&&!harness)} onClick={()=>{void open(true,undefined,unified?targetFor(value.active_id)!:harness??null);}}><Icon name="split"/></button>}<button className="icon-button" title={value.vertical?'Рядом':'Друг под другом'} aria-label={value.vertical?'Рядом':'Друг под другом'} data-command="orientation" disabled={!value.split_id} onClick={()=>save({...value,vertical:!value.vertical})}><Icon name={value.vertical?'split':'rows'}/></button>{value.split_id&&<button className="icon-button" title="Один pane" aria-label="Один pane" data-command="single" onClick={()=>save({...value,split_id:null})}><Icon name="panels"/></button>}</div>;
  const tabs = <div className="workspace-tab-scroll"><div ref={tabStrip} className="terminal-tabs" role="tablist" aria-label="Terminal tabs">{value.tabs.map(t=><div key={t.id} className="workspace-tab" data-active={value.active_id===t.id} draggable onDragStart={event=>{dragged.current=t.id;event.dataTransfer.effectAllowed='move';event.dataTransfer.setData('text/plain',t.id);}} onDragEnd={()=>{dragged.current=null;}} onDragOver={event=>{if(dragged.current){event.preventDefault();event.dataTransfer.dropEffect='move';}}} onDrop={event=>{event.preventDefault();if(dragged.current)save({...current.current,tabs:moveTab(current.current.tabs,dragged.current,t.id)});dragged.current=null;}}><button role="tab" aria-selected={value.active_id===t.id} title={`${t.title} · Перетащите или Alt+Shift+←/→ для перемещения`} onKeyDown={event=>{if(event.altKey&&event.shiftKey&&(event.key==='ArrowLeft'||event.key==='ArrowRight')){event.preventDefault();const index=value.tabs.findIndex(tab=>tab.id===t.id),target=value.tabs[index+(event.key==='ArrowLeft'?-1:1)];if(target)save({...current.current,tabs:moveTab(current.current.tabs,t.id,target.id)});}}} onClick={()=>{save({...value,active_id:t.id,split_id:value.split_id===t.id?null:value.split_id});onActivate?.(targetFor(t.id)??null);}}><Icon name={t.launch?.kind==='harness'?'play':'terminal'} size={14}/><span>{t.title}{(!compact||sessions.find(s=>s.id===t.id)?.state!=='running')&&` · ${sessions.find(s=>s.id===t.id)?.state??'stopped'}`}</span></button><button aria-label={`Закрыть ${t.title}`} disabled={busy||launching||!confirmed||!connected} title="Закрыть вкладку и её процесс" onClick={()=>{void close(t.id);}}><Icon name="close" size={13}/></button></div>)}</div>{unified&&<NewTabMenu installations={installations!} disabled={disabled||value.tabs.length>=8} create={target=>void open(false,undefined,target)} settings={onSettings}/>}</div>;
  return <section className={`terminal-workspace${compact?' compact':''}${unified?' unified':''}`} aria-label="Terminal workspace">{compact||unified?<div className="terminal-strip">{tabs}{toolbar}</div>:<>{toolbar}{tabs}</>}
    {loading&&<p role="status">Loading — owned terminals…</p>}{!connected&&<p role="alert">Disconnected: ввод отключён, состояние процессов не подтверждено.</p>}{error&&<p role="alert">{error}</p>}
    {unlisted.map(s=><button key={s.id} onClick={()=>save({...value,tabs:[...value.tabs,{id:s.id,title:`${s.shell} · reattach`}],active_id:s.id})}>Подключить существующий {s.shell}</button>)}
    <div className={`terminal-panes${value.vertical?' vertical':''}`}>{value.tabs.map(tab=>{const visible=tab.id===value.active_id||tab.id===value.split_id;return <div key={tab.id} className="terminal-slot" hidden={!visible} style={{order:tab.id===value.active_id?0:1}}>{pane(tab.id,visible)}</div>;})}</div>
    {!value.tabs.length&&<div className="empty-selection"><Icon name="terminal" size={28}/><p>{unified?'Создайте вкладку через «+»':'Откройте терминал в этом worktree.'}</p><p className="muted">{unified?'Выберите harness или обычный терминал.':'PowerShell в Windows / Bash в выбранной WSL distro. Команды выполняются только после вашего ввода.'}</p></div>}
  </section>;
}


