import type { SVGProps } from 'react';

const paths = {
  copy: 'M9 9h12v12H9zM15 9V3H3v12h6',
  branch: 'M6 6v12M18 6v3a6 6 0 0 1-6 6H6',
  panels: 'M8 4v16M4 4h16v16H4z',
  folder: 'M3 7V5h6l2 2h10v12H3z',
  worktree: 'm12 3 9 9-9 9-9-9zM8 12h8m-4-4v8',
  chevron: 'm9 5 7 7-7 7',
  plus: 'M12 5v14M5 12h14',
  search: 'M21 21l-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',
  settings: 'm9 3-1 3-3 1-2 3 2 2v3l2 3 3-1 2 2 3-1 1-3 3-1 2-3-2-2V8l-2-3-3 1-2-2zM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0',
  bell: 'M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  refresh: 'M20 7v5h-5M4 17v-5h5M6 6a8 8 0 0 1 13 3M18 18a8 8 0 0 1-13-3',
  file: 'M14 3H5v18h14V8zM14 3v6h5M8 13h8M8 17h6',
  changes: 'M8 4H4v16h16v-4M9 8h12M15 2v12M9 17h5',
  terminal: 'm5 7 5 5-5 5m8 0h6',
  chat: 'M4 4h16v12H9l-5 4zM8 8h8M8 12h5',
  close: 'm6 6 12 12M6 18 18 6',
  grid: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
  monitor: 'M3 4h18v13H3zM8 21h8m-4-4v4',
  server: 'M3 3h18v7H3zM3 14h18v7H3zM7 6h.01M7 17h.01',
  split: 'M3 4h18v16H3zM12 4v16',
  rows: 'M3 4h18v16H3zM3 12h18',
  expand: 'm8 9 4-4 4 4m-8 6 4 4 4-4M4 12h16',
  check: 'm5 12 4 4L19 6',
  shield: 'm12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6zM9 12l2 2 4-4',
  send: 'm3 3 18 9-18 9 4-9zM7 12h14',
  stop: 'M6 6h12v12H6z',
  play: 'm8 4 12 8-12 8z',
} as const;
export type IconName = keyof typeof paths;
export function Icon({ name, size = 16, className = '', ...props }: SVGProps<SVGSVGElement> & { name: IconName; size?: number }) {
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={name === 'more' ? 3.5 : 1.65} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={`icon ${className}`} {...props}><path d={paths[name]}/>{name === 'branch' && <><circle cx="6" cy="4" r="2"/><circle cx="6" cy="20" r="2"/><circle cx="18" cy="4" r="2"/></>}</svg>;
}
