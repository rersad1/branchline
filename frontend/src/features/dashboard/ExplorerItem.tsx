import { useRef, type ReactNode } from 'react';
import { ActionMenu } from '../../components/ui/action-menu';

/** The visible ellipsis and row context menu share the exact same actions. */
export function ExplorerItem({label,className,actions,children}:{label:string;className:string;actions:ReactNode;children:ReactNode}) {
  const row=useRef<HTMLLIElement>(null);
  return <li ref={row} className={className}>{children}<div className="explorer-actions"><ActionMenu label={label} contextTarget={row}>{actions}</ActionMenu></div></li>;
}
