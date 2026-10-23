import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { Icon, type IconName } from './icon';

/** A small, dismissible action group. Portal avoids clipping inside editor/terminal panes. */
export function ActionMenu({ label, icon = 'more', children, badge, contextTarget, command }: { label: string; icon?: IconName; children: ReactNode; badge?: number; contextTarget?: RefObject<HTMLElement|null>; command?:string }) {
  const [open, setOpen] = useState(false);
  const [point,setPoint]=useState<{x:number;y:number}|null>(null);
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const close = (returnFocus = false) => { setOpen(false); if (returnFocus) trigger.current?.focus(); };
  useLayoutEffect(() => {
    if (!open || !panel.current || !trigger.current) return;
    const anchor = trigger.current.getBoundingClientRect(), menu = panel.current;
    menu.style.left = `${Math.max(8, Math.min(point?.x ?? anchor.right - menu.offsetWidth, innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(point?.y ?? anchor.bottom + 6, innerHeight - menu.offsetHeight - 8))}px`;
    (menu.querySelector<HTMLButtonElement>('button:not(:disabled)') ?? menu).focus();
  }, [open,point]);
  useEffect(()=>{
    const target=contextTarget?.current;if(!target)return;
    const context=(event:MouseEvent)=>{event.preventDefault();event.stopPropagation();setPoint(event.clientX||event.clientY?{x:event.clientX,y:event.clientY}:null);setOpen(true);};
    target.addEventListener('contextmenu',context);return()=>target.removeEventListener('contextmenu',context);
  },[contextTarget]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!panel.current?.contains(event.target as Node) && !trigger.current?.contains(event.target as Node)) close(); };
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(true); }
      else if (event.key === 'Tab') {
        const buttons = [...(panel.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
        if (!buttons.length || (event.shiftKey ? buttons[0] : buttons.at(-1)) === document.activeElement) close(true);
      }
      else if (event.ctrlKey && event.altKey) close();
    };
    const resize = () => close();
    const scroll=(event:Event)=>{if(!panel.current?.contains(event.target as Node))close();};
    const visibility = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => {
      if (trigger.current && !trigger.current.getClientRects().length) close();
    });
    if (trigger.current) visibility?.observe(trigger.current);
    document.addEventListener('pointerdown', outside); document.addEventListener('keydown', key, true); window.addEventListener('resize', resize);
    window.addEventListener('scroll',scroll,true);
    return () => { visibility?.disconnect(); document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', key, true); window.removeEventListener('resize', resize);window.removeEventListener('scroll',scroll,true); };
  }, [open]);
  return <>
    <button type="button" ref={trigger} className="icon-button menu-trigger" data-command={command} aria-label={label} title={label} aria-expanded={open} aria-controls={open ? id : undefined} onClick={() => {setPoint(null);setOpen(!open);}} onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault();setPoint(null); setOpen(true); } }}><Icon name={icon}/>{!!badge && <span className="notification-count">{badge}</span>}</button>
    {open && createPortal(<div id={id} ref={panel} tabIndex={-1} className="action-popover" role="group" aria-label={label} onBlur={event => { if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget as Node) && event.relatedTarget !== trigger.current) close(); }} onClick={event => { if ((event.target as Element).closest('button:not(:disabled)')) close(); }} onKeyDown={event => {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === 'ArrowDown' ? (current + 1) % buttons.length : event.key === 'ArrowUp' ? (current - 1 + buttons.length) % buttons.length : event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : null;
      if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
    }}>{children}</div>, document.body)}
  </>;
}
