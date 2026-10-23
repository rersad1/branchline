import { useRef, useState } from 'react';

export const MIN_EXPLORER_WIDTH = 220;
export const MAX_EXPLORER_WIDTH = 360;

export function ExplorerResize({ width, preview, commit, label = 'Ширина Explorer', min = MIN_EXPLORER_WIDTH, max = MAX_EXPLORER_WIDTH, reset = 280 }: { width: number; preview: (width: number | null) => void; commit: (width: number) => void; label?: string; min?: number; max?: number; reset?: number }) {
  const clamp = (value: number) => Math.round(Math.max(min, Math.min(max, value)));
  const drag = useRef<{ id: number; x: number; start: number; width: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  function cancel() { drag.current = null; setDragging(false); preview(null); }
  return <div role="separator" aria-label={label} aria-orientation="vertical"
    aria-valuemin={min} aria-valuemax={max} aria-valuenow={width}
    tabIndex={0} className={`explorer-resize${dragging ? ' dragging' : ''}`}
    title="Потяните границу · двойной щелчок — сброс ширины"
    onPointerDown={event => {
      if (event.button !== 0 || drag.current) return;
      event.preventDefault(); event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { id: event.pointerId, x: event.clientX, start: width, width }; setDragging(true);
    }}
    onPointerMove={event => {
      if (drag.current?.id !== event.pointerId) return;
      drag.current.width = clamp(drag.current.start + event.clientX - drag.current.x);
      preview(drag.current.width);
    }}
    onPointerUp={event => {
      if (drag.current?.id !== event.pointerId) return;
      const next = drag.current.width; const start = drag.current.start; cancel();
      event.currentTarget.releasePointerCapture(event.pointerId);
      if (next !== start) commit(next);
    }}
    onPointerCancel={cancel} onLostPointerCapture={cancel}
    onDoubleClick={() => { cancel(); if (width !== clamp(reset)) commit(clamp(reset)); }}
    onKeyDown={event => {
      if (event.key === 'Escape' && drag.current) { event.currentTarget.releasePointerCapture(drag.current.id); cancel(); return; }
      if (drag.current) return;
      const next = event.key === 'ArrowLeft' ? clamp(width - 10) : event.key === 'ArrowRight' ? clamp(width + 10) : event.key === 'Home' ? min : event.key === 'End' ? max : null;
      if (next === null) return;
      event.preventDefault(); if (next !== width) commit(next);
    }}/ >;
}
