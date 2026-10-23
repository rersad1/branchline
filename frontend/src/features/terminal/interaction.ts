import type { Terminal } from '@xterm/xterm';

/** OpenCode's Escape interrupts a turn; ETX is an application-exit binding. */
export function terminalKeyAction(event: KeyboardEvent, selected: boolean, opencode: boolean) {
  if (!event.ctrlKey || event.altKey || event.metaKey || event.getModifierState('AltGraph')) return null;
  const key = event.code || event.key.toLowerCase();
  if (key === 'KeyC' || key === 'c') {
    if (event.shiftKey || selected) return 'copy';
    if (opencode) return 'interrupt';
  }
  if ((key === 'KeyV' || key === 'v') && event.shiftKey) return 'paste';
  return null;
}

export function attachTerminalInteraction(terminal: Terminal, opencode: boolean, send: (data: string) => void, error: (text: string) => void, shell?: {nativePaste:()=>boolean; host:HTMLElement}) {
  let alive = true;
  let wheelPixels = 0, wheelAt = -Infinity, wheelSeen = -Infinity;
  const unsafeMultiline=(text:string)=>!!shell&&!terminal.modes.bracketedPasteMode&&/[\r\n]/.test(text);
  const blockedPaste=()=>error('Оболочка не подтвердила безопасную многострочную вставку. Включите shell integration / PSReadLine или вставляйте строки по отдельности.');
  // Native PSReadLine Paste inserts the OS clipboard as one editable buffer in
  // versions without bracketed paste. Only explicit user paste invokes it.
  const pasteEvent=(event:ClipboardEvent)=>{
    if(!shell)return;
    if(shell.nativePaste()){event.preventDefault();event.stopImmediatePropagation();send('\x16');}
    else if(unsafeMultiline(event.clipboardData?.getData('text/plain')??'')){event.preventDefault();event.stopImmediatePropagation();blockedPaste();}
  };
  shell?.host.addEventListener('paste',pasteEvent,true);
  terminal.attachCustomKeyEventHandler(event => {
    const action = terminalKeyAction(event, terminal.hasSelection(), opencode);
    // Use physical control chords consistently across keyboard layouts. WebView's
    // translated key/keypress can otherwise insert Cyrillic letters instead of ETX.
    const control = !action && event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey
      && !event.getModifierState('AltGraph') && event.code!=='KeyV' && /^Key[A-Z]$/.test(event.code)
      ? String.fromCharCode(event.code.charCodeAt(3) - 64) : null;
    if (!action && !control) return true;
    event.preventDefault();
    event.stopPropagation();
    if (event.type !== 'keydown') return false;
    if (control) send(control);
    if (action === 'interrupt') send('\x1b');
    if (action === 'copy') {
      const text = terminal.getSelection();
      if (text) void navigator.clipboard?.writeText(text).catch(() => { if (alive) error('Не удалось скопировать текст.'); });
    }
    if (action === 'paste') {
      if(shell?.nativePaste())send('\x16');
      else if (!navigator.clipboard?.readText) error('Используйте Ctrl+V для вставки.');
      else void navigator.clipboard.readText().then(text => { if (alive){if(unsafeMultiline(text))blockedPaste();else terminal.paste(text);} }).catch(() => { if (alive) error('Не удалось прочитать буфер обмена. Используйте Ctrl+V.'); });
    }
    return false;
  });
  terminal.attachCustomWheelEventHandler(event => {
    event.stopPropagation();
    if (!opencode) return true;
    terminal.focus();
    terminal.scrollToBottom();
    // Preserve the application's mouse protocol. Without it, use line keys:
    // Ctrl+Alt+Y/E are OpenCode's line-scroll bindings; arrows edit prompt history.
    if (terminal.modes.mouseTrackingMode !== 'none') return true;
    event.preventDefault();
    const now = performance.now();
    if (now - wheelSeen > 200 || Math.sign(wheelPixels) !== Math.sign(event.deltaY)) wheelPixels = 0;
    wheelSeen = now;
    wheelPixels += event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 120 : 1);
    if (Math.abs(wheelPixels) >= 40 && now - wheelAt >= 40) {
      send(wheelPixels < 0 ? '\x1b\x19' : '\x1b\x05');
      wheelPixels = 0; wheelAt = now;
    }
    return false;
  });
  return () => { alive = false;shell?.host.removeEventListener('paste',pasteEvent,true); };
}
