// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { Dashboard, treeName, branchLabel } from '../src/features/dashboard/Dashboard';
import { ApiClient, type Project, type Snapshot, type Worktree } from '../src/api';

const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.forEach(client => client.clear()); clients.length = 0; vi.useRealTimers(); });
const projects: Project[] = ['alpha', 'beta'].map(id => ({ environment_id: 'env-a', id, name: `Project ${id}`, root_path: `/home/${id}`, git_common_dir: `/home/${id}/.git`, base_ref: null, created_at: new Date().toISOString() }));
function makeTree(projectId: string, overrides: Partial<Worktree> = {}): Worktree {
  return { environment_id: 'env-a', id: projectId, project_id: projectId, path: `/home/${projectId}`, branch: 'main', head: 'abc', is_main: true,
    is_bare: false, is_detached: false, is_locked: false, lock_reason: null, is_prunable: false, prune_reason: null, is_available: true, availability_error: null,
    git_status: { staged: 0, unstaged: 0, untracked: 0, conflicts: 0, files: [], last_successful_refresh_at: new Date().toISOString(), error: null }, ...overrides };
}
function mount(snapshots: Snapshot[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  clients.push(client);
  const api = new ApiClient('not-used');
  vi.spyOn(api, 'worktrees').mockImplementation(async id => snapshots.find(snap => snap.project_id === id)!);
  snapshots.forEach(snap => client.setQueryData(['worktrees', snap.environment_id, snap.project_id], snap));
  const renderDashboard = (failed = false) => <QueryClientProvider client={client}><Dashboard api={api} projects={projects} transportFailed={failed}/></QueryClientProvider>;
  const rendered = render(renderDashboard());
  return { client, ...rendered, renderDashboard };
}
const snap = (worktree: Worktree): Snapshot => ({ environment_id: 'env-a', project_id: worktree.project_id, worktrees: [worktree], error: null, last_successful_refresh_at: new Date().toISOString() });
describe('Dashboard', () => {
  it('keeps identical project/worktree IDs and paths separate across environments', () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    clients.push(client);
    const a = { ...projects[0], name: 'Distro A', environment_id: 'env-a' };
    const b = { ...a, name: 'Distro B', environment_id: 'env-b' };
    const first = snap(makeTree(a.id));
    const second = { ...first, environment_id: 'env-b', worktrees: [{ ...first.worktrees[0], environment_id: 'env-b', branch: 'other' }] };
    client.setQueryData(['worktrees', 'env-a', a.id], first);
    client.setQueryData(['worktrees', 'env-b', b.id], second);
    render(<QueryClientProvider client={client}><Dashboard api={new ApiClient('unused')} projects={[a, b]} transportFailed={false}/></QueryClientProvider>);
    fireEvent.click(screen.getByRole('button', { name: /other/ }));
    expect(within(screen.getByRole('region', { name: 'Worktree overview' })).getByRole('heading', { name: 'Distro B / alpha' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /main/ }));
    expect(within(screen.getByRole('region', { name: 'Worktree overview' })).getByRole('heading', { name: 'Distro A / alpha' })).toBeTruthy();
  });
  it('renders nested projects, selection, detached/missing and independent Unknown agent state', () => {
    const alpha = makeTree('alpha');
    alpha.git_status.unstaged = 3;
    const beta = makeTree('beta', { is_detached: true, branch: null, is_available: false, is_prunable: true });
    const { rerender, renderDashboard } = mount([snap(alpha), snap(beta)]);
    expect(screen.getByRole('button', { name: 'Project alpha' })).toBeTruthy(); expect(screen.getByRole('button', { name: 'Project beta' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /main/ }));
    const overview = screen.getByRole('region', { name: 'Worktree overview' });
    expect(within(overview).getByText('Unstaged: 3')).toBeTruthy();
    expect(within(overview).queryByText('Running', { exact: true })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Detached ·/ }));
    expect(within(overview).getByText('Недоступно / missing')).toBeTruthy();
    expect(within(overview).queryByText('Clean')).toBeNull();
    rerender(renderDashboard(true));
    expect(screen.queryByText('Clean')).toBeNull();
  });
  it('ages cached zero counts without another network response', async () => {
    vi.useFakeTimers();
    mount([snap(makeTree('alpha')), snap(makeTree('beta'))]);
    expect(screen.getAllByText('Clean')).toHaveLength(2);
    await act(async () => { vi.advanceTimersByTime(21_000); });
    expect(screen.queryByText('Clean')).toBeNull();
    expect(screen.getAllByText('Устарело / Unknown')).toHaveLength(2);
  });
  it('isolates discovery errors and clears a removed selection', async () => {
    const alpha = snap(makeTree('alpha')); const beta = snap(makeTree('beta'));
    alpha.error = { code: 'unavailable', message: 'Project missing' };
    const { client } = mount([alpha, beta]);
    expect(screen.getAllByText('Clean')).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: /beta.*main/ }));
    expect(screen.getByRole('heading', { name: 'Project beta / beta' })).toBeTruthy();
    await act(async () => { client.setQueryData(['worktrees', 'env-a', 'beta'], { ...beta, worktrees: [] }); await new Promise(resolve => setTimeout(resolve, 5)); });
    expect(screen.getByRole('heading', { name: 'Project beta / Repository' })).toBeTruthy();
    expect(screen.getByText('Worktree не обнаружены.')).toBeTruthy();
  });
});

it('uses basename, separate branch/SHA and is_main for Primary', () => {
  const tree = makeTree('alpha', { path: 'C:\\repos\\linked', branch: 'main', is_main: false });
  expect(treeName(tree)).toBe('linked');
  expect(branchLabel(tree)).toBe('main');
  expect(branchLabel({ ...tree, is_detached: true, head: '123456789abcdef' })).toBe('Detached · 123456789a');
  mount([snap(tree), snap(makeTree('beta'))]);
  const row = screen.getByRole('button', { name: /linked.*main/ });
  expect(within(row).getByText('linked')).toBeTruthy();
  expect(within(row).queryByText('Primary')).toBeNull();
  expect(screen.getAllByText('Primary')).toHaveLength(1);
});
it('preserves an unavailable persisted selection and isolates a failed distro', () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
  const select = vi.fn();
  const healthy = snap(makeTree('beta')); client.setQueryData(['worktrees', 'env-a', 'beta'], healthy);
  render(<QueryClientProvider client={client}><Dashboard api={new ApiClient('unused')} projects={[projects[1]]}
    environments={[{ environment: { id: 'env-a', kind: 'windows', distro: null }, state: 'connected', error: null, authoritative: true }, { environment: { id: 'missing', kind: 'wsl', distro: 'Unavailable' }, state: 'error', error: { code: 'startup_failed', message: 'Distro missing' }, authoritative: false }]}
    selection={{ environment_id: 'missing', project_id: 'old-project', worktree_id: 'old-tree' }} onSelection={select}/></QueryClientProvider>);
  expect(screen.getByRole('heading', { name: 'old-project / Worktree недоступен' })).toBeTruthy();
  expect(screen.getByText(/Выбранный контекст недоступен/)).toBeTruthy();
  expect(screen.getByText('Clean')).toBeTruthy();
  expect(select).not.toHaveBeenCalled();
});
