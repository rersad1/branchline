import { describe, expect, it } from 'vitest';
import { gitView, MAX_STATUS_AGE_MS } from '../src/features/dashboard/status';
import type { Snapshot, Worktree } from '../src/api';
export const now = Date.parse('2026-10-13T15:00:00Z');
export function tree(overrides: Partial<Worktree> = {}): Worktree {
  return {
    environment_id: 'env-a', id: 'tree', project_id: 'project', path: '/home/project', branch: 'main', head: 'abc123',
    is_main: true, is_bare: false, is_detached: false, is_locked: false, lock_reason: null,
    is_prunable: false, prune_reason: null, is_available: true, availability_error: null,
    git_status: { staged: 0, unstaged: 0, untracked: 0, conflicts: 0, files: [], error: null, last_successful_refresh_at: new Date(now).toISOString() },
    ...overrides,
  };
}
export function snapshot(worktree = tree()): Snapshot {
  return { environment_id: worktree.environment_id, project_id: worktree.project_id, worktrees: [worktree], error: null, last_successful_refresh_at: new Date(now).toISOString() };
}
describe('Git observations', () => {
  it('shows Clean only for fresh, available, fully known successful counts', () => {
    const worktree = tree();
    expect(gitView(worktree, snapshot(), false, now).label).toBe('Clean');
    expect(gitView(worktree, snapshot(), false, now + MAX_STATUS_AGE_MS + 1).current).toBe(false);
  });
  it.each(['staged', 'unstaged', 'untracked', 'conflicts'] as const)('does not conflate %s with Clean', field => {
    const worktree = tree(); worktree.git_status[field] = 2;
    expect(gitView(worktree, snapshot(), false, now).label).toBe(field === 'conflicts' ? 'Conflicts' : 'Changes');
    worktree.git_status[field] = null;
    expect(gitView(worktree, snapshot(), false, now).current).toBe(false);
  });
  it('invalidates zero counts on status error, snapshot error, availability error and disconnect', () => {
    const worktree = tree(); const snap = snapshot();
    const problem = { code: 'git_failed', message: 'Failed' };
    for (const [candidate, candidateSnapshot, disconnected] of [
      [{ ...worktree, git_status: { ...worktree.git_status, error: problem } }, snap, false],
      [worktree, { ...snap, error: problem }, false],
      [{ ...worktree, is_available: false }, snap, false],
      [{ ...worktree, availability_error: problem }, snap, false],
      [worktree, snap, true],
      [{ ...worktree, is_bare: true }, snap, false],
    ] as const) expect(gitView(candidate, candidateSnapshot, disconnected, now).current).toBe(false);
    expect(worktree.git_status.staged).toBe(0); // Keep last observation, not a new clean assertion.
  });
  it.each([null, 'invalid', '2026-10-13T14:59:00Z', '2026-10-13T15:01:00Z'])('rejects missing/invalid/stale/future timestamp %s on both layers', value => {
    const worktree = tree(); const snap = snapshot();
    expect(gitView({ ...worktree, git_status: { ...worktree.git_status, last_successful_refresh_at: value } }, snap, false, now).current).toBe(false);
    expect(gitView(worktree, { ...snap, last_successful_refresh_at: value }, false, now).current).toBe(false);
  });
  it('rejects negative, non-finite or fractional counts', () => {
    for (const staged of [-1, NaN, Infinity, 0.5]) {
      const worktree = tree(); worktree.git_status.staged = staged;
      expect(gitView(worktree, snapshot(), false, now).current).toBe(false);
    }
  });
});
