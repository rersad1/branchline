// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { ExplorerResize } from '../src/features/dashboard/ExplorerResize';
import { commitLabel } from '../src/features/dashboard/Dashboard';

afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
function mount() {
  vi.stubGlobal('PointerEvent', class extends MouseEvent {
    pointerId: number;
    constructor(type: string, init: PointerEventInit) { super(type, init); this.pointerId = init.pointerId ?? 1; }
  });
  const commit = vi.fn();
  function Layout() {
    const [width, setWidth] = useState(280);
    const [preview, setPreview] = useState<number | null>(null);
    return <ExplorerResize width={preview ?? width} preview={setPreview} commit={next => { commit(next); setWidth(next); }}/>;
  }
  render(<Layout/>);
  const separator = screen.getByRole('separator', { name: 'Ширина Explorer' });
  separator.setPointerCapture = vi.fn(); separator.releasePointerCapture = vi.fn();
  return { separator, commit };
}
it('previews pointer movement, clamps bounds, ignores other pointers and saves exactly once on release', () => {
  const { separator, commit } = mount();
  fireEvent.pointerDown(separator, { pointerId: 7, button: 0, clientX: 280 });
  fireEvent.pointerMove(separator, { pointerId: 8, clientX: 0 });
  expect(separator.getAttribute('aria-valuenow')).toBe('280');
  fireEvent.pointerMove(separator, { pointerId: 7, clientX: 323 });
  expect(separator.getAttribute('aria-valuenow')).toBe('323'); expect(commit).not.toHaveBeenCalled();
  fireEvent.pointerMove(separator, { pointerId: 7, clientX: 900 });
  expect(separator.getAttribute('aria-valuenow')).toBe('360');
  fireEvent.pointerUp(separator, { pointerId: 7 });
  expect(commit.mock.calls).toEqual([[360]]);
  fireEvent.lostPointerCapture(separator); expect(separator.getAttribute('aria-valuenow')).toBe('360');
});
it('cancels drag on Escape or pointer cancellation without saving a partial width', () => {
  const { separator, commit } = mount();
  for (const cancel of ['escape', 'pointercancel']) {
    fireEvent.pointerDown(separator, { pointerId: 1, button: 0, clientX: 280 });
    fireEvent.pointerMove(separator, { pointerId: 1, clientX: 10 });
    expect(separator.getAttribute('aria-valuenow')).toBe('220');
    if (cancel === 'escape') fireEvent.keyDown(separator, { key: 'Escape' }); else fireEvent.pointerCancel(separator, { pointerId: 1 });
    expect(separator.getAttribute('aria-valuenow')).toBe('280');
  }
  expect(commit).not.toHaveBeenCalled();
});
it('supports keyboard sizing and double-click reset within the existing persisted bounds', () => {
  const { separator, commit } = mount();
  fireEvent.keyDown(separator, { key: 'ArrowRight' }); expect(commit).toHaveBeenLastCalledWith(290);
  fireEvent.keyDown(separator, { key: 'Home' }); expect(commit).toHaveBeenLastCalledWith(220);
  fireEvent.keyDown(separator, { key: 'ArrowLeft' }); expect(commit).toHaveBeenCalledTimes(2);
  fireEvent.keyDown(separator, { key: 'End' }); expect(commit).toHaveBeenLastCalledWith(360);
  fireEvent.doubleClick(separator); expect(commit).toHaveBeenLastCalledWith(280);
});
it('labels unborn zero HEAD as no commit', () => {
  expect(commitLabel({ head: '0'.repeat(40) })).toBe('Нет commit');
  expect(commitLabel({ head: 'abcdef1234567890' })).toBe('abcdef1234');
});
