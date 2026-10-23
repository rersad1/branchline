import type { Installation } from '../../desktop';
import { ActionMenu } from '../../components/ui/action-menu';
import { Icon, type IconName } from '../../components/ui/icon';

export function NewTabMenu({installations,disabled,create,settings,label='Новая вкладка',icon='plus'}:{installations:Installation[];disabled:boolean;create:(installation:Installation|null)=>void;settings?:()=>void;label?:string;icon?:IconName}) {
  return <ActionMenu label={label} icon={icon} command={label==='Новая вкладка'?'new-terminal':undefined}>
    <button disabled={disabled} onClick={()=>create(null)}><Icon name="terminal"/>Терминал</button>
    <p className="new-tab-heading">Harness</p>
    {installations.map(i=><button key={i.id} title={i.executable} disabled={disabled} onClick={()=>create(i)}><Icon name="play"/><span>{i.name==='opencode'?'OpenCode':i.name}{i.version?` · ${i.version}`:''}<small>{i.executable}</small></span></button>)}
    {!installations.length&&<p className="muted new-tab-heading">Нет обнаруженных harness</p>}
    {settings&&<button onClick={settings}><Icon name="settings"/>Настроить harness…</button>}
  </ActionMenu>;
}
