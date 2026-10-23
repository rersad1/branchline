import { useEffect, useRef, type ReactNode } from 'react';
import { Icon } from '../../components/ui/icon';

/** Native top-layer dialog: Escape, focus containment and focus return are browser-owned. */
export function AgentDialog({ title, close, children }: { title: string; close: () => void; children: ReactNode }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const element = dialog.current; element?.showModal(); return () => element?.close(); }, []);
  return <dialog ref={dialog} className="agent-dialog" aria-label={title} onCancel={event => { event.preventDefault(); close(); }}>
    <header><h2>{title}</h2><button className="icon-button" aria-label="Закрыть окно" onClick={close}><Icon name="close"/></button></header>
    <div className="agent-dialog-content">{children}</div>
  </dialog>;
}
