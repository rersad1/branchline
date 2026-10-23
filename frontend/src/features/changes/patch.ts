export interface Hunk {
  header: string; oldStart: number; newStart: number;
  original: string; modified: string; oldLines: number; newLines: number;
}
// Only actual patch lines become models. Omitted context is never invented.
export function parseHunks(patch: string): Hunk[] {
  const result: Hunk[] = [];
  let current: { header: string; oldStart: number; newStart: number; old: string[]; next: string[]; oldNoNewline: boolean; newNoNewline: boolean; last: string } | null = null;
  const flush = () => {
    if (current) result.push({ header: current.header, oldStart: current.oldStart, newStart: current.newStart,
      original: current.old.join('\n') + (current.old.length && !current.oldNoNewline ? '\n' : ''), modified: current.next.join('\n') + (current.next.length && !current.newNoNewline ? '\n' : ''), oldLines: current.old.length, newLines: current.next.length });
  };
  for (const line of patch.split('\n')) {
    const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (match) { flush(); current = { header: line, oldStart: Number(match[1]), newStart: Number(match[2]), old: [], next: [], oldNoNewline: false, newNoNewline: false, last: '' }; }
    else if (current) {
      if (line.startsWith(' ')) { current.old.push(line.slice(1)); current.next.push(line.slice(1)); }
      else if (line.startsWith('-')) current.old.push(line.slice(1));
      else if (line.startsWith('+')) current.next.push(line.slice(1));
      else if (line.startsWith('\\ No newline')) {
        if (current.last === '-' || current.last === ' ') current.oldNoNewline = true;
        if (current.last === '+' || current.last === ' ') current.newNoNewline = true;
      }
      if ([' ', '-', '+'].includes(line[0])) current.last = line[0];
    }
  }
  flush(); return result;
}
export function languageFor(path: string): string {
  const extension = path.split('.').pop()?.toLowerCase() ?? '';
  return ({ ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', rs: 'rust', py: 'python', kt: 'kotlin', kts: 'kotlin', java: 'java', cs: 'csharp', c: 'c', h: 'c', cpp: 'cpp', go: 'go', rb: 'ruby', sh: 'shell', ps1: 'powershell', json: 'json', css: 'css', html: 'html', xml: 'xml', md: 'markdown', yaml: 'yaml', yml: 'yaml', toml: 'ini', sql: 'sql' } as Record<string, string>)[extension] ?? 'plaintext';
}
