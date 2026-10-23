/** Application bindings deliberately avoid terminal control keys and editor shortcuts. */
export interface Command { id: string; label: string; shortcut?: string; enabled: boolean; run: () => void }
export const bindings: Record<string,string> = {
  palette: 'p', overview: '1', changes: '2', terminal: '3', explorer: 'e',
  focus: 'f', next: 'ArrowRight', previous: 'ArrowLeft', preferences: ',',
};
export function binding(event: Pick<KeyboardEvent,'ctrlKey'|'altKey'|'shiftKey'|'metaKey'|'key'|'isComposing'> & {getModifierState?:(key:string)=>boolean}) {
  if (!event.ctrlKey || !event.altKey || event.shiftKey || event.metaKey || event.isComposing) return;
  // AltGr is text input, never an application command.
  if (event.getModifierState?.('AltGraph')) return;
  return Object.keys(bindings).find(id => bindings[id].toLowerCase() === event.key.toLowerCase());
}
export const shortcut = (id: string) => bindings[id] ? `Ctrl+Alt+${bindings[id]}` : undefined;
export function commandButton(id: string) {
  return [...document.querySelectorAll<HTMLButtonElement>(`button[data-command="${id}"]`)].find(button =>
    !button.disabled && !button.closest('[hidden]') && (!!button.closest('details') || button.getClientRects().length > 0));
}
export const buttonCommands = [
  ['harness', 'Настроить harness'], ['diagnostics', 'Диагностика и worktree'],
  ['new-terminal', 'Новый терминал'], ['split', 'Terminal: новый split'],
  ['single', 'Terminal: один pane'], ['orientation', 'Terminal: сменить ориентацию'],
  ['history', 'Harness: история сессий'], ['load', 'Harness: загрузить сессию'],
] as const;
