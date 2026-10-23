import type { ITheme } from '@xterm/xterm';

// Display-only OSC 633 property. Never route a reported path back to backend scope.
export function shellDirectory(data: string): string | null | undefined {
  if (!data.startsWith('P;Cwd=')) return undefined;
  if (data.length > 32768) return null;
  try {
    const path = decodeURIComponent(data.slice(6));
    if (!path || /[\x00-\x1f\x7f-\x9f]/.test(path)) return null;
    return path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith('\\\\') ? path : null;
  } catch { return null; }
}

export function terminalTheme(light: boolean): ITheme {
  return light ? {
    background:'#F8FAFD',foreground:'#172330',cursor:'#0066B5',cursorAccent:'#FFFFFF',
    selectionBackground:'#B7D5F4',selectionForeground:'#172330',
    black:'#172330',red:'#B3263E',green:'#216E40',yellow:'#805F0B',blue:'#005CAA',magenta:'#853493',cyan:'#086B78',white:'#526478',
    brightBlack:'#526478',brightRed:'#AA1836',brightGreen:'#146537',brightYellow:'#765800',brightBlue:'#005CAA',brightMagenta:'#853493',brightCyan:'#086B78',brightWhite:'#172330',
  } : {
    background:'#11151B',foreground:'#E2E8F2',cursor:'#79A9FF',cursorAccent:'#11151B',
    selectionBackground:'#36557D',selectionForeground:'#FFFFFF',
    black:'#536176',red:'#EF7D8D',green:'#72DC9B',yellow:'#EAC45D',blue:'#79A9FF',magenta:'#D69DEB',cyan:'#69CCDD',white:'#D5DDE8',
    brightBlack:'#8996A9',brightRed:'#FF9AA8',brightGreen:'#93EAB2',brightYellow:'#F6D580',brightBlue:'#A1C2FF',brightMagenta:'#E6B8F6',brightCyan:'#91DFEA',brightWhite:'#FFFFFF',
  };
}
