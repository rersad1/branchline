import { useEffect, useRef, useState } from 'react';
import * as monaco from 'monaco-editor/esm/vs/editor/editor.api.js';
import 'monaco-editor/esm/vs/basic-languages/typescript/typescript.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/javascript/javascript.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/rust/rust.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/python/python.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/kotlin/kotlin.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/java/java.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/csharp/csharp.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/cpp/cpp.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/go/go.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/ruby/ruby.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/shell/shell.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/powershell/powershell.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/css/css.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/html/html.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/xml/xml.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/markdown/markdown.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/yaml/yaml.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/ini/ini.contribution.js';
import 'monaco-editor/esm/vs/basic-languages/sql/sql.contribution.js';
import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker.js?worker';
import 'monaco-editor/esm/vs/language/json/monaco.contribution.js';
import JsonWorker from 'monaco-editor/esm/vs/language/json/json.worker.js?worker';
import { languageFor, parseHunks, type Hunk } from './patch';
import type { Draft } from './drafts';

const workingModels = new Map<string, { original: monaco.editor.ITextModel; modified: monaco.editor.ITextModel; view: monaco.editor.IDiffEditorViewState | null; endings: Map<number, readonly string[]>; lastEndings: readonly string[] }>();
export function MonacoWorking({ draft, sideBySide, readOnly, onChange, onSave }: { draft: Draft; sideBySide: boolean; readOnly: boolean; onChange: (text: string, changes: readonly monaco.editor.IModelContentChange[], endings?: readonly string[]) => void; onSave: () => void }) {
  const container = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneDiffEditor | undefined>(undefined);
  const callbacks = useRef({ onChange, onSave }); callbacks.current = { onChange, onSave };
  useEffect(() => {
    let models = workingModels.get(draft.key);
    if (!models) {
      models = { original: monaco.editor.createModel(draft.original, languageFor(draft.request.path)), modified: monaco.editor.createModel(draft.text, languageFor(draft.request.path)), view: null, endings: new Map(), lastEndings: draft.raw.match(/\r\n|\r|\n/g) ?? [] };
      models.modified.setEOL(monaco.editor.EndOfLineSequence.LF);
      models.endings.set(models.modified.getAlternativeVersionId(), models.lastEndings);
      workingModels.set(draft.key, models);
    }
    const instance = monaco.editor.createDiffEditor(container.current!, {
      theme: 'branchline', readOnly, originalEditable: false, renderSideBySide: sideBySide,
      useInlineViewWhenSpaceIsLimited: false, automaticLayout: true, minimap: { enabled: false },
      fontFamily: 'Cascadia Code, Consolas, monospace', fontSize: 13, lineHeight: 20,
      scrollBeyondLastLine: false, renderMarginRevertIcon: false, renderGutterMenu: false,
      hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 8, revealLineCount: 20 },
      ignoreTrimWhitespace: false,
      // Monaco 0.55 word-highlighter rejects an unhandled delayed promise on fast dispose.
      occurrencesHighlight: 'off',
    });
    editor.current = instance; instance.setModel(models);
    instance.getOriginalEditor().updateOptions({ ariaLabel: 'Исходный файл', lineNumbers: sideBySide ? 'on' : 'off' });
    instance.getModifiedEditor().updateOptions({ ariaLabel: 'Рабочий файл' });
    if (models.view) instance.restoreViewState(models.view);
    const listener = models.modified.onDidChangeContent(event => {
      const version = models!.modified.getAlternativeVersionId();
      callbacks.current.onChange(models!.modified.getValue(monaco.editor.EndOfLinePreference.LF), event.changes, event.isUndoing || event.isRedoing ? models!.endings.get(version) : undefined);
      const endings = draft.raw.match(/\r\n|\r|\n/g) ?? [];
      if (endings.length !== models!.lastEndings.length || endings.some((value,index) => value !== models!.lastEndings[index])) models!.lastEndings = endings;
      // Share unchanged EOL sequences across keystrokes; retain provenance with Monaco's undo versions.
      models!.endings.set(version, models!.lastEndings);
    });
    instance.getModifiedEditor().addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => callbacks.current.onSave());
    return () => { models!.view = instance.saveViewState(); listener.dispose(); instance.dispose(); editor.current = undefined; };
  }, [draft.key]);
  useEffect(() => {
    editor.current?.updateOptions({ renderSideBySide: sideBySide, readOnly });
    // Inline has one gutter for current-file coordinates; removed rows have no current line.
    editor.current?.getOriginalEditor().updateOptions({ lineNumbers: sideBySide ? 'on' : 'off' });
  }, [sideBySide, readOnly]);
  const original = (draft.external?.text ?? draft.original).replace(/\r\n|\r/g, '\n');
  useEffect(() => {
    const models = workingModels.get(draft.key); if (!models) return;
    if (models.original.getValue(monaco.editor.EndOfLinePreference.LF) !== original) models.original.setValue(original);
    if (models.modified.getValue(monaco.editor.EndOfLinePreference.LF) !== draft.text) {
      models.modified.pushEditOperations([], [{ range: models.modified.getFullModelRange(), text: draft.text }], () => null);
    }
    // A refresh/accept can change only raw EOLs, leaving normalized Monaco text equal.
    // Its current undo version must then represent those newly accepted endings.
    const endings = draft.raw.match(/\r\n|\r|\n/g) ?? [];
    if (endings.length !== models.lastEndings.length || endings.some((value,index) => value !== models.lastEndings[index])) models.lastEndings = endings;
    models.endings.set(models.modified.getAlternativeVersionId(), models.lastEndings);
  }, [draft.key, draft.text, draft.raw, original]);
  return <div ref={container} className="monaco-working" aria-label="Редактируемый рабочий diff"/>;
}

self.MonacoEnvironment = { getWorker: (_id, label) => label === 'json' ? new JsonWorker() : new EditorWorker() };
monaco.editor.defineTheme('branchline', {
  base: 'vs-dark', inherit: true, rules: [], colors: {
    'editor.background': '#12171C', 'editor.foreground': '#E6EDF5', 'editorLineNumber.foreground': '#8290A4',
    'diffEditor.insertedLineBackground': '#17392C', 'diffEditor.removedLineBackground': '#3A202B',
    'diffEditor.insertedTextBackground': '#24543F', 'diffEditor.removedTextBackground': '#592B3C',
  },
});
function HunkEditor({ hunk, language, sideBySide }: { hunk: Hunk; language: string; sideBySide: boolean }) {
  const container = useRef<HTMLDivElement>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    const original = monaco.editor.createModel(hunk.original, language);
    const modified = monaco.editor.createModel(hunk.modified, language);
    let editor: monaco.editor.IStandaloneDiffEditor | undefined;
    try {
      editor = monaco.editor.createDiffEditor(container.current!, {
        theme: 'branchline', readOnly: true, originalEditable: false, renderSideBySide: sideBySide,
        useInlineViewWhenSpaceIsLimited: false, automaticLayout: true, minimap: { enabled: false },
        fontFamily: 'Cascadia Code, Consolas, monospace', fontSize: 13, lineHeight: 20,
        glyphMargin: false, lineDecorationsWidth: 4,
        lineNumbersMinChars: Math.max(2, String(Math.max(hunk.oldStart + hunk.oldLines, hunk.newStart + hunk.newLines)).length),
        scrollBeyondLastLine: false, renderIndicators: true, renderOverviewRuler: false,
        enableSplitViewResizing: true, renderMarginRevertIcon: false, renderGutterMenu: false,
        stickyScroll: { enabled: false }, folding: false, contextmenu: false,
        hideUnchangedRegions: { enabled: false }, diffAlgorithm: 'advanced', ignoreTrimWhitespace: false,
        occurrencesHighlight: 'off',
      });
      editor.setModel({ original, modified });
      editor.getOriginalEditor().updateOptions({ lineNumbers: sideBySide ? n => n <= hunk.oldLines ? String(hunk.oldStart + n - 1) : '' : 'off' });
      editor.getModifiedEditor().updateOptions({ lineNumbers: n => n <= hunk.newLines ? String(hunk.newStart + n - 1) : '' });
      // Keep absolute old coordinates in split view; inline numbers belong to the new file.
    } catch (e) { setError(`Editor error: ${(e as Error).message}`); }
    return () => { editor?.dispose(); original.dispose(); modified.dispose(); };
  }, [hunk, language, sideBySide]);
  return <section className="diff-hunk"><code className="hunk-header">{hunk.header}</code>{error && <p role="alert">{error}</p>}<div ref={container} className="monaco-hunk" aria-label="Read-only diff hunk"/></section>;
}
export default function MonacoPatch({ patch, path, sideBySide }: { patch: string; path: string; sideBySide: boolean }) {
  const [index, setIndex] = useState(0);
  const hunks = useRef<{ patch: string; values: Hunk[] } | null>(null);
  if (hunks.current?.patch !== patch) hunks.current = { patch, values: parseHunks(patch) };
  const values = hunks.current.values;
  const current = Math.min(index, Math.max(0, values.length - 1));
  return <>{patch.includes('\\ No newline at end of file') && <p className="diff-message">No newline at end of file — один из вариантов не заканчивается переводом строки.</p>}
    {values.length ? <>{values.length > 1 && <div className="context-control"><label htmlFor="diff-hunk">Hunk</label><select id="diff-hunk" value={current} onChange={e => setIndex(Number(e.target.value))}>{values.map((hunk, i) => <option key={i} value={i}>{i + 1}/{values.length} · −{hunk.oldStart} +{hunk.newStart}</option>)}</select><button disabled={current === 0} onClick={() => setIndex(current - 1)}>Предыдущий</button><button disabled={current === values.length - 1} onClick={() => setIndex(current + 1)}>Следующий</button></div>}<HunkEditor hunk={values[current]} language={languageFor(path)} sideBySide={sideBySide}/></> : <div className="diff-message">Нет изменённых строк: изменение имени, file mode или пустой файл.<pre>{patch}</pre></div>}</>;
}
