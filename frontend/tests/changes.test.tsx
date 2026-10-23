// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Changes } from '../src/features/changes/Changes';
import { ApiClient, ApiError, type DiffResponse, type Project, type Worktree } from '../src/api';
import { parseHunks } from '../src/features/changes/patch';
vi.mock('../src/features/changes/MonacoPatch', () => ({ default: ({ patch }: { patch: string }) => <pre>{patch}</pre> }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
const project = { id: 'p', environment_id: 'win', base_ref: 'main' } as Project;
const tree = { id: 't', environment_id: 'win', project_id: 'p' } as Worktree;
const files = ['one.ts', 'dir/two.rs'].map((path, index) => ({ id: String(index), path, original_path: null, status: 'M', additions: 1, deletions: 1, binary: false, submodule: false }));
const response = (overrides: Partial<DiffResponse> = {}): DiffResponse => ({ environment_id: 'win', project_id: 'p', worktree_id: 't', mode: 'unstaged', base_ref: null, base_commit: null, head_commit: 'head', state: 'ready', observed_at: new Date().toISOString(), context_lines: 3, files, selected_file: null, ...overrides });
it('parses actual hunk offsets, additions/deletions and no-newline markers without fabricated context', () => {
  const hunks = parseHunks('diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -56,2 +56,3 @@ fn\n before\n-old\n+new\n+extra\n\\ No newline at end of file\n@@ -100,0 +102,1 @@\n+last\n');
  expect(hunks).toEqual([{ header: '@@ -56,2 +56,3 @@ fn', oldStart: 56, newStart: 56, original: 'before\nold\n', modified: 'before\nnew\nextra', oldLines: 2, newLines: 3 }, { header: '@@ -100,0 +102,1 @@', oldStart: 100, newStart: 102, original: '', modified: 'last\n', oldLines: 0, newLines: 1 }]);
  expect(parseHunks('@@ -0,0 +1,1 @@\n+\n')[0].modified).toBe('\n');
  const eof = parseHunks('@@ -1,1 +1,1 @@\n-same\n+same\n\\ No newline at end of file\n')[0];
  expect(eof.original).toBe('same\n'); expect(eof.modified).toBe('same');
});
it('requests metadata then only the selected file and expands context without changing scope', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => response({ ...request, selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: `patch ${request.file_id} context ${request.context_lines}` } : null }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  await screen.findByRole('button', { name: /one.ts/ });
  expect(diff).toHaveBeenCalledTimes(1); expect(diff.mock.calls[0][1].file_id).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: /two.rs/ }));
  await screen.findByText('patch 1 context 3');
  expect(diff.mock.calls[1][1].file_id).toBe('1');
  fireEvent.click(screen.getByRole('button', { name: 'Параметры diff' }));
  fireEvent.click(screen.getByRole('button', { name: 'Расширить контекст' }));
  await screen.findByText('patch 1 context 23');
  expect(diff.mock.calls[2][1]).toMatchObject({ project_id: 'p', worktree_id: 't', mode: 'unstaged', file_id: '1', base_ref: null, context_lines: 23 });
});
it('shows untracked line counts in the list before selection and keeps unknown statistics explicit', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockResolvedValue(response({ files: [
    { ...files[0], status: '?', additions: 37, deletions: 0 },
    { ...files[1], status: '?', additions: null, deletions: 0, binary: null },
  ] }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  const counted = await screen.findByRole('button', { name: /one.ts/ });
  expect(counted.textContent).toContain('+37');
  expect(counted.textContent).toContain('−0');
  expect(screen.getByRole('button', { name: /two.rs/ }).textContent).toContain('+—');
  expect(screen.getByText('Untracked · статистика недоступна')).toBeTruthy();
  expect(screen.queryByText(/counts после выбора/)).toBeNull();
  expect(diff).toHaveBeenCalledTimes(1);
  expect(diff.mock.calls[0][1].file_id).toBeNull();
});
it('resizes Changed Files independently without reloading metadata or selected diff', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => response({ ...request, selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: 'selected patch' } : null }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  fireEvent.click(await screen.findByRole('button', { name: /one.ts/ }));
  await screen.findByText('selected patch');
  const splitter = screen.getByRole('separator', { name: 'Ширина Changed Files' });
  fireEvent.keyDown(splitter, { key: 'ArrowRight' });
  expect(splitter.getAttribute('aria-valuenow')).toBe('280');
  fireEvent.keyDown(splitter, { key: 'Home' });
  expect(splitter.getAttribute('aria-valuenow')).toBe('180');
  fireEvent.keyDown(splitter, { key: 'End' });
  expect(splitter.getAttribute('aria-valuenow')).toBe('520');
  fireEvent.doubleClick(splitter);
  expect(splitter.getAttribute('aria-valuenow')).toBe('270');
  expect(diff).toHaveBeenCalledTimes(2);
  expect(screen.getByText('selected patch')).toBeTruthy();
});
it('discards late file/mode/base/environment responses, including requests whose transport ignores abort', async () => {
  const api = new ApiClient('unused'); let finishOld!: (value: DiffResponse) => void;
  vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => request.file_id === '0' ? new Promise(resolve => { finishOld = resolve; }) : response({ ...request, files: request.mode === 'branch' ? [] : files, selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: 'current patch' } : null }));
  const props = { api, project, tree, connected: true, unavailable: null };
  const view = render(<Changes {...props}/>);
  fireEvent.click(await screen.findByRole('button', { name: /one.ts/ }));
  await waitFor(() => expect(finishOld).toBeTypeOf('function'));
  fireEvent.click(screen.getByRole('button', { name: /two.rs/ }));
  await screen.findByText('current patch');
  await act(async () => finishOld(response({ selected_file: { file_id: '0', state: 'text', patch: 'foreign late patch' } })));
  expect(screen.queryByText('foreign late patch')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Branch vs base' }));
  await screen.findByText('Нет изменений в выбранном режиме.');
  expect(screen.queryByText('current patch')).toBeNull();
  fireEvent.change(screen.getByLabelText('Base'), { target: { value: 'other' } }); fireEvent.click(screen.getByRole('button', { name: 'Применить' }));
  await waitFor(() => expect(api.diff).toHaveBeenLastCalledWith('win', expect.objectContaining({ mode: 'branch', base_ref: 'other', file_id: null }), expect.any(AbortSignal)));
  view.rerender(<Changes {...props} tree={{ ...tree, environment_id: 'ubuntu' }}/>);
  await waitFor(() => expect(api.diff).toHaveBeenLastCalledWith('ubuntu', expect.anything(), expect.any(AbortSignal)));
});
it('keeps explicit fallbacks and disconnected/error/missing data separate from an empty successful diff', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockResolvedValue(response({ files: [], state: 'no_base' }));
  const props = { api, project, tree, connected: true, unavailable: null };
  const view = render(<Changes {...props}/>);
  await screen.findByText(/No base:/); expect(screen.queryByText('Нет изменений в выбранном режиме.')).toBeNull();
  view.rerender(<Changes {...props} connected={false}/>);
  expect(screen.getByRole('status').textContent).toContain('Disconnected');
  diff.mockRejectedValue(new ApiError('disconnected'));
  view.rerender(<Changes {...props}/>);
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Rust backend недоступен'));
  expect(screen.queryByText('Clean')).toBeNull();
});
it.each(['binary', 'large_file', 'conflict', 'submodule'] as const)('shows selected %s fallback without a fake patch', async state => {
  const api = new ApiClient('unused');
  vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => response({ selected_file: request.file_id ? { file_id: request.file_id, state, patch: null } : null }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  fireEvent.click(await screen.findByRole('button', { name: /one.ts/ }));
  await screen.findByText(new RegExp(state === 'large_file' ? 'Large file:' : state === 'binary' ? 'Binary:' : state === 'conflict' ? 'Conflict:' : 'Submodule:'));
});

it('preserves selection and last-known patch during refresh/error and reloads that file on recovery', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => response({ selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: 'old patch' } : null }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  fireEvent.click(await screen.findByRole('button', { name: /one.ts/ })); await screen.findByText('old patch');
  let finish!: (data: DiffResponse) => void;
  diff.mockImplementation(async (_env, request) => request.file_id ? Promise.reject(new ApiError('error', 409)) : new Promise(resolve => { finish = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh Changes' }));
  expect(screen.getByRole('status').textContent).toContain('Loading');
  expect(screen.getByRole('status').textContent).toContain('last-known');
  expect(screen.getByText('old patch')).toBeTruthy();
  expect(screen.getByRole('button', { name: /one.ts/ }).getAttribute('aria-pressed')).toBe('true');
  await act(async () => finish(response()));
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Error'));
  expect(screen.getByText('old patch')).toBeTruthy();
  diff.mockImplementation(async (_env, request) => response({ selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: 'new patch' } : null }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh Changes' }));
  await screen.findByText('new patch'); expect(screen.queryByText('old patch')).toBeNull();
  expect(diff).toHaveBeenLastCalledWith('win', expect.objectContaining({ file_id: '0' }), expect.any(AbortSignal));
});

it('reloads on meaningful Git changes and reconnect, skips timestamp-only polls and clears removed selection', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockImplementation(async (_env, request) => response({ selected_file: request.file_id ? { file_id: request.file_id, state: 'text', patch: 'patch' } : null }));
  const currentTree = { ...tree, head: 'head', git_status: { staged: 0, unstaged: 2, untracked: 0, conflicts: 0, files: [], error: null, last_successful_refresh_at: new Date().toISOString() } };
  const props = { api, project, tree: currentTree, connected: true, unavailable: null };
  const view = render(<Changes {...props}/>);
  fireEvent.click(await screen.findByRole('button', { name: /one.ts/ })); await screen.findByText('patch');
  const before = diff.mock.calls.length;
  view.rerender(<Changes {...props} tree={{ ...currentTree, git_status: { ...currentTree.git_status, last_successful_refresh_at: new Date(Date.now() + 1000).toISOString() } }}/>);
  expect(diff).toHaveBeenCalledTimes(before);
  view.rerender(<Changes {...props} tree={{ ...currentTree, head: 'new head' }}/>);
  expect(screen.getByRole('status').textContent).toContain('last-known');
  await waitFor(() => expect(diff.mock.calls.length).toBe(before + 2));
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('автосохранение'));
  view.rerender(<Changes {...props} connected={false}/>);
  expect(screen.getByRole('status').textContent).toContain('Disconnected');
  diff.mockResolvedValue(response({ files: [] }));
  view.rerender(<Changes {...props}/>);
  await screen.findByText('Выбранный файл больше не входит в текущий diff. Выберите другой файл.');
  expect(screen.queryByText('patch')).toBeNull();
});

it.each(['bare', 'unborn', 'no_base', 'ambiguous_base'] as const)('renders %s instead of an empty successful diff', async state => {
  const api = new ApiClient('unused'); vi.spyOn(api, 'diff').mockResolvedValue(response({ state, files: [] }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  await screen.findByText(new RegExp(state === 'bare' ? 'Bare repository:' : state === 'unborn' ? 'Unborn:' : state === 'no_base' ? 'No base:' : 'Неоднозначный merge-base:'));
  expect(screen.queryByText('Нет изменений в выбранном режиме.')).toBeNull();
});

it('never presents old or implausibly future observations as current', async () => {
  const api = new ApiClient('unused');
  const diff = vi.spyOn(api, 'diff').mockResolvedValue(response({ observed_at: new Date(Date.now() - 21000).toISOString(), files: [] }));
  render(<Changes api={api} project={project} tree={tree} connected unavailable={null}/>);
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Stale'));
  expect(screen.queryByText('Нет изменений в выбранном режиме.')).toBeNull();
  diff.mockResolvedValue(response({ observed_at: new Date(Date.now() + 60000).toISOString(), files: [] }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh Changes' }));
  await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Stale'));
});

it('keeps the editor fully readable when its snapshot ages, including offline last-known data',async()=>{
 const api=new ApiClient('unused');vi.spyOn(api,'diff').mockResolvedValue(response({observed_at:new Date(Date.now()-60000).toISOString()}));
 const props={api,project,tree,connected:true,unavailable:null};const r=render(<Changes {...props}/>);
 await waitFor(()=>expect(screen.getByRole('status').textContent).toContain('Stale'));
 expect(r.container.querySelector('.diff-scroll')?.classList.contains('diff-stale')).toBe(false);
 r.rerender(<Changes {...props} connected={false}/>);
 expect(screen.getByRole('status').textContent).toContain('Disconnected');
 expect(r.container.querySelector('.diff-scroll')?.classList.contains('diff-stale')).toBe(false);
});
