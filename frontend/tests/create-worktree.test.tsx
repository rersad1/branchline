// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ApiClient, ApiError, type CreateResponse, type Project } from '../src/api';
import { CreateWorktree } from '../src/features/dashboard/CreateWorktree';
const project = { id: 'repo', environment_id: 'win', name: 'Fixture', root_path: 'C:\\source', base_ref: 'main' } as Project;
const result = { environment_id: 'win', project_id: 'repo', path: 'C:\\external', branch: 'feat/new', git_created: true, branch_exists: true, path_exists: true, worktree_id: 'new', snapshot: null, warning: null } as CreateResponse;
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
function mount(operation: () => Promise<CreateResponse>) {
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
  const api = new ApiClient('unused'); const create = vi.spyOn(api, 'createWorktree').mockImplementation(operation); const created = vi.fn();
  const mounted = render(<CreateWorktree api={api} projects={[project]} environments={[{ environment: { id: 'win', kind: 'windows', distro: null }, state: 'connected', error: null }]} close={vi.fn()} created={created}/>);
  fireEvent.change(screen.getByLabelText('Среда worktree'), { target: { value: 'win' } });
  fireEvent.change(screen.getByLabelText('Repository'), { target: { value: 'repo' } });
  fireEvent.change(screen.getByLabelText('Новая ветка'), { target: { value: 'feat/new' } });
  fireEvent.change(screen.getByLabelText('Полный путь (Windows)'), { target: { value: 'C:\\external' } });
  return { ...mounted, create, created };
}
it('requires explicit environment/repo, prevents duplicate submission, shows actual success', async () => {
  let resolve!: (r: CreateResponse) => void;
  const { create, created } = mount(() => new Promise(r => { resolve = r; }));
  const submit = screen.getByRole('button', { name: 'Создать worktree' });
  fireEvent.click(submit); fireEvent.click(submit);
  expect(create).toHaveBeenCalledTimes(1);
  expect((submit as HTMLButtonElement).disabled).toBe(true);
  await act(async () => resolve(result));
  expect(created).toHaveBeenCalledWith(result);
  expect(screen.getByText('Worktree создан Git.')).toBeTruthy();
});
it('discards a late response after the form context unmounts', async () => {
  let resolve!: (r: CreateResponse) => void;
  const { unmount, created } = mount(() => new Promise(r => { resolve = r; }));
  fireEvent.click(screen.getByRole('button', { name: 'Создать worktree' })); unmount();
  await act(async () => resolve(result)); expect(created).not.toHaveBeenCalled();
});
it('allows correcting a known preflight error; lost transport response blocks resubmission', async () => {
  const { create } = mount(async () => { throw new ApiError('error', undefined, 'create_base'); });
  fireEvent.click(screen.getByRole('button', { name: 'Создать worktree' }));
  await act(async () => {});
  expect((screen.getByRole('button', { name: 'Создать worktree' }) as HTMLButtonElement).disabled).toBe(false);
  create.mockRejectedValueOnce(new ApiError('disconnected'));
  fireEvent.click(screen.getByRole('button', { name: 'Создать worktree' }));
  await act(async () => {});
  expect(screen.queryByRole('button', { name: 'Создать worktree' })).toBeNull();
  expect(screen.getByText(/Результат не подтверждён/)).toBeTruthy();
});
