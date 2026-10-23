// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ActionMenu } from '../src/components/ui/action-menu';
afterEach(cleanup);
it('skips disabled actions, supports directional navigation and restores focus on Escape', () => {
  render(<ActionMenu label="Actions"><button disabled>Unavailable</button><button>First</button><button>Second</button></ActionMenu>);
  const trigger=screen.getByRole('button',{name:'Actions'});
  fireEvent.click(trigger);expect(document.activeElement).toBe(screen.getByText('First'));
  fireEvent.keyDown(screen.getByRole('group'),{key:'ArrowDown'});expect(document.activeElement).toBe(screen.getByText('Second'));
  fireEvent.keyDown(document,{key:'Escape'});expect(document.activeElement).toBe(trigger);
  expect(trigger.getAttribute('aria-expanded')).toBe('false');expect(screen.queryByRole('group')).toBeNull();
});
it('dismisses outside clicks and executes an action once', () => {
  const run=vi.fn();render(<><button>Outside</button><ActionMenu label="Actions"><button onClick={run}>Run</button></ActionMenu></>);
  fireEvent.click(screen.getByRole('button',{name:'Actions'}));fireEvent.pointerDown(screen.getByText('Outside'));expect(screen.queryByRole('group')).toBeNull();
  fireEvent.click(screen.getByRole('button',{name:'Actions'}));fireEvent.click(screen.getByText('Run'));expect(run).toHaveBeenCalledTimes(1);expect(screen.queryByRole('group')).toBeNull();
});
it('can focus and dismiss a group with no enabled actions', () => {
  render(<ActionMenu label="Empty"><p>No events</p><button disabled>Clear</button></ActionMenu>);
  fireEvent.click(screen.getByRole('button',{name:'Empty'}));expect(document.activeElement).toBe(screen.getByRole('group'));
  fireEvent.keyDown(document,{key:'Escape'});expect(document.activeElement).toBe(screen.getByRole('button',{name:'Empty'}));
});
