import { expect, it } from 'vitest';
import { shellDirectory, terminalTheme } from '../src/features/terminal/shellIntegration';

it('decodes only bounded absolute shell paths as inert display data',()=>{
  for(const path of ['C:\\Юникод с пробелом\\file', 'D:\\', '/tmp/тест пробел/../outside', '//host/share', '/'+ 'long/'.repeat(1000)]) {
    expect(shellDirectory('P;Cwd='+encodeURIComponent(path))).toBe(path);
  }
  for(const value of ['', '%ZZ', 'relative', '%2Ftmp%1B%5D', '%2Ftmp%00', 'x'.repeat(32769)])expect(shellDirectory('P;Cwd='+value)).toBeNull();
  expect(shellDirectory('P;Other=value')).toBeUndefined();
});
it('provides dedicated ANSI, selection and cursor palettes for both themes',()=>{
  for(const light of [false,true]) {
    const theme=terminalTheme(light);
    expect(theme.foreground).not.toBe(theme.background);
    expect(theme.cursor).not.toBe(theme.background);
    expect(theme.selectionBackground).not.toBe(theme.background);
    expect(theme.red).not.toBe(theme.green);
  }
  expect(terminalTheme(false).background).not.toBe(terminalTheme(true).background);
});
