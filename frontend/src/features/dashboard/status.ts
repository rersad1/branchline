import type { Snapshot, Worktree } from '../../api';

// Above the backend's 5s refresh interval and 10s Git timeout; still a bounded
// observation, never a claim of an atomic/live Git snapshot.
export const MAX_STATUS_AGE_MS = 20_000;
export function fresh(timestamp: string | null, now: number) {
  if (!timestamp) return false;
  const age = now - Date.parse(timestamp);
  return Number.isFinite(age) && age >= -5000 && age <= MAX_STATUS_AGE_MS;
}
export function gitView(tree: Worktree, snapshot: Snapshot, transportFailed: boolean, now: number) {
  const status = tree.git_status;
  const counts = [status.staged, status.unstaged, status.untracked, status.conflicts];
  const validCounts = counts.every(count => count !== null && Number.isSafeInteger(count) && count >= 0);
  const reason = transportFailed ? 'Нет подтверждённого соединения; последние известные данные.' :
    snapshot.error ? `${snapshot.error.code}: ${snapshot.error.message}` :
    !tree.is_available || tree.availability_error ? `Недоступно: ${tree.availability_error?.message ?? 'worktree не подтверждён'}` :
    tree.is_bare ? 'Bare repository: Git status рабочего дерева отсутствует.' :
    status.error ? `${status.error.code}: ${status.error.message}` :
    !fresh(snapshot.last_successful_refresh_at, now) || !fresh(status.last_successful_refresh_at, now) ? 'Timestamp отсутствует, некорректен или данные старше 20 секунд.' :
    !validCounts ? 'Git status ещё не получен или счётчики некорректны.' : null;
  const current = reason === null;
  return {
    current, reason,
    label: !current ? (tree.is_bare ? 'Status N/A' : 'Устарело / Unknown') :
      status.conflicts! > 0 ? 'Conflicts' : counts.some(count => count! > 0) ? 'Changes' : 'Clean',
  };
}
export function timestamp(value: string | null) {
  if (!value || !Number.isFinite(Date.parse(value))) return 'Нет успешного обновления';
  return new Date(value).toLocaleString('ru-RU', { timeZoneName: 'short' });
}
