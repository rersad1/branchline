import { useState, type ReactNode } from 'react';

/** Mount on first visit; retain local drafts, editor and terminal state until context changes. */
export function WorkspaceView({ active, children }: { active: boolean; children: ReactNode }) {
  const [visited, setVisited] = useState(active);
  if (active && !visited) setVisited(true);
  return active || visited ? <div className="worktree-view" hidden={!active}>{children}</div> : null;
}
