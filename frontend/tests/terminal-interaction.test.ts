// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import type { Terminal } from '@xterm/xterm';
import { attachTerminalInteraction, terminalKeyAction } from '../src/features/terminal/interaction';

function fixture(opencode = true, shellMode=false, native=false) {
  let key!: (event: KeyboardEvent) => boolean, wheel!: (event: WheelEvent) => boolean;
  const terminal = {
    selected: '', modes: { mouseTrackingMode: 'none',bracketedPasteMode:true },
    attachCustomKeyEventHandler: (handler: typeof key) => { key = handler; },
    attachCustomWheelEventHandler: (handler: typeof wheel) => { wheel = handler; },
    hasSelection: () => !!terminal.selected, getSelection: () => terminal.selected,
    paste: vi.fn(), focus: vi.fn(), scrollToBottom: vi.fn(),
  };
  const send = vi.fn(), error = vi.fn();
  const host=document.createElement('div');
  const dispose = attachTerminalInteraction(terminal as unknown as Terminal, opencode, send, error,shellMode?{host,nativePaste:()=>native}:undefined);
  return { terminal, send, error, dispose,host, key: (event: KeyboardEvent) => key(event), wheel: (event: WheelEvent) => wheel(event) };
}
const ctrl = (code = 'KeyC', extra: KeyboardEventInit = {}) => new KeyboardEvent('keydown', { code, ctrlKey: true, cancelable: true, ...extra });
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it('interrupts OpenCode with Escape, copies selection, and preserves shell Ctrl+C', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  const f = fixture(), shell = fixture(false);
  const event = ctrl('KeyC', { key: 'с' }); // Physical shortcut also works with Cyrillic layout.
  expect(f.key(event)).toBe(false); expect(event.defaultPrevented).toBe(true); expect(f.send).toHaveBeenCalledWith('\x1b');
  expect(shell.key(ctrl())).toBe(false); expect(shell.send).toHaveBeenCalledWith('\x03');
  expect(shell.key(ctrl('KeyC',{key:'с'}))).toBe(false);expect(shell.send).toHaveBeenCalledWith('\x03');
  f.terminal.selected = 'selected text'; expect(f.key(ctrl())).toBe(false);
  expect(writeText).toHaveBeenCalledWith('selected text'); expect(f.send).toHaveBeenCalledTimes(1);
  expect(f.key(new KeyboardEvent('keyup', { code: 'KeyC', ctrlKey: true }))).toBe(false);
  expect(f.send).toHaveBeenCalledTimes(1);
  f.terminal.selected = ''; f.key(ctrl('KeyC', { shiftKey: true })); expect(f.send).toHaveBeenCalledTimes(1);
});
it('maps physical shell controls in Cyrillic layout without stealing AltGr or duplicating keyup',()=>{
 const f=fixture(false);
 for(const [code,key,data] of [['KeyD','в','\x04'],['KeyL','д','\x0c'],['KeyR','к','\x12']]) {
   expect(f.key(ctrl(code,{key}))).toBe(false);expect(f.send).toHaveBeenLastCalledWith(data);
 }
 expect(f.key(new KeyboardEvent('keyup',{code:'KeyD',key:'в',ctrlKey:true}))).toBe(false);
 expect(f.send).toHaveBeenCalledTimes(3);
 expect(f.key(ctrl('KeyR',{key:'к',altKey:true}))).toBe(true);
});
it('leaves AltGr and native Ctrl+V alone, uses bracketed paste, and ignores late clipboard reads', async () => {
  expect(terminalKeyAction(ctrl('KeyC', { altKey: true }), false, true)).toBe(null);
  expect(terminalKeyAction(ctrl('KeyV'), false, true)).toBe(null);
  let resolve!: (text: string) => void;
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { readText: () => new Promise<string>(done => { resolve = done; }) } });
  const f = fixture(); expect(f.key(ctrl('KeyV', { shiftKey: true }))).toBe(false);
  expect(f.key(ctrl('KeyV'))).toBe(true);
  resolve('pasted\ntext'); await Promise.resolve(); expect(f.terminal.paste).toHaveBeenCalledWith('pasted\ntext');
  f.key(ctrl('KeyV', { shiftKey: true })); f.dispose(); resolve('late'); await Promise.resolve();
  expect(f.terminal.paste).toHaveBeenCalledTimes(1);
});
it('routes OpenCode wheel to its mouse protocol or bounded line keys instead of xterm history', () => {
  const f = fixture(), shell = fixture(false);
  const event = new WheelEvent('wheel', { deltaY: 120, cancelable: true });
  expect(f.wheel(event)).toBe(false); expect(event.defaultPrevented).toBe(true);
  expect(f.send).toHaveBeenCalledWith('\x1b\x05'); expect(f.terminal.focus).toHaveBeenCalled();
  vi.spyOn(performance,'now').mockReturnValue(performance.now()+50);
  f.wheel(new WheelEvent('wheel', { deltaY: -120 })); expect(f.send).toHaveBeenLastCalledWith('\x1b\x19');
  f.terminal.modes.mouseTrackingMode = 'any';
  expect(f.wheel(new WheelEvent('wheel', { deltaY: 120 }))).toBe(true); expect(f.send).toHaveBeenCalledTimes(2);
  expect(shell.wheel(event)).toBe(true); expect(shell.terminal.scrollToBottom).not.toHaveBeenCalled();
});
it('uses native PSReadLine Paste only on explicit user paste and blocks unsafe shell fallback',async()=>{
 const native=fixture(false,true,true);
 native.key(ctrl('KeyV',{shiftKey:true}));expect(native.send).toHaveBeenCalledWith('\x16');expect(native.terminal.paste).not.toHaveBeenCalled();
 const event=new Event('paste',{cancelable:true});native.host.dispatchEvent(event);expect(event.defaultPrevented).toBe(true);expect(native.send).toHaveBeenCalledTimes(2);
 native.dispose();native.host.dispatchEvent(new Event('paste'));expect(native.send).toHaveBeenCalledTimes(2);
 Object.defineProperty(navigator,'clipboard',{configurable:true,value:{readText:async()=> 'one\ntwo'}});
 const basic=fixture(false,true);basic.terminal.modes.bracketedPasteMode=false;
 basic.key(ctrl('KeyV',{shiftKey:true}));await Promise.resolve();expect(basic.terminal.paste).not.toHaveBeenCalled();expect(basic.error).toHaveBeenCalled();
 const bash=fixture(false,true);bash.key(ctrl('KeyV',{shiftKey:true}));await Promise.resolve();expect(bash.terminal.paste).toHaveBeenCalledWith('one\ntwo');
});

it('bounds fast wheel bursts and accumulates small touchpad deltas',()=>{
 let now=100;vi.spyOn(performance,'now').mockImplementation(()=>now);
 const f=fixture();
 for(let n=0;n<100;n++)f.wheel(new WheelEvent('wheel',{deltaY:120}));
 expect(f.send).toHaveBeenCalledTimes(1);
 now+=50;f.wheel(new WheelEvent('wheel',{deltaY:120}));expect(f.send).toHaveBeenCalledTimes(2);
 now+=300;for(let n=0;n<7;n++)f.wheel(new WheelEvent('wheel',{deltaY:5}));
 expect(f.send).toHaveBeenCalledTimes(2);
 f.wheel(new WheelEvent('wheel',{deltaY:5}));expect(f.send).toHaveBeenCalledTimes(3);
});
