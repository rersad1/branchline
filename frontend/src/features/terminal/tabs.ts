import type { TerminalLayout } from '../../desktop';

type Tab = TerminalLayout['tabs'][number];

/** Generated titles follow the visible order; legacy custom titles stay intact. */
export function numberTabs(tabs: Tab[]): Tab[] {
  return tabs.map((tab, index) => {
    if (!tab.launch && !/^(Терминал|OpenCode|Terminal|PowerShell|Bash|Windows PowerShell) \d+$/.test(tab.title)) return tab;
    const name = tab.launch?.kind === 'shell' ? 'Терминал' : tab.title.replace(/ \d+$/, '');
    const title = `${name} ${index + 1}`;
    return title === tab.title ? tab : { ...tab, title };
  });
}

export function moveTab(tabs: Tab[], id: string, target: string): Tab[] {
  const from = tabs.findIndex(tab => tab.id === id), to = tabs.findIndex(tab => tab.id === target);
  if (from < 0 || to < 0 || from === to) return tabs;
  const next = [...tabs];
  next.splice(to, 0, next.splice(from, 1)[0]);
  return numberTabs(next);
}
