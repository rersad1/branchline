# Windows notifications

In **Настройки**, enable **Attention по подтверждённым событиям** and then
**Windows notifications**. Both preferences are saved. Windows must also allow
notifications for Agent Workspace Manager (Branchline).

Branchline monitors connected agent integrations across registered worktrees,
including those outside the selected panel. ACP permission requests and OpenCode
HTTP/SSE `permission.asked` and `question.asked` events raise **Агент ожидает ответа**.
The waiting banner uses Windows' long duration. Errors and confirmed ACP turn
completion also notify. Question replies/rejections do not create another alert.

Clicking a notification restores the running app and opens the exact environment,
project, worktree and agent session. The app checks that the target still exists
before switching. A stopped session, removed worktree or disconnected environment
shows an unavailable message without launching an agent.

Windows receives only the Branchline name, a fixed status and a random opaque
activation token. Prompts, answers, tool titles, repository paths and session IDs
are not included. Session targets stay in memory. Closing the app hides retained
toasts, revokes its COM activator and restores its prior activation registration;
notifications do not restart the app.

Attention remains available inside Branchline when Windows denies notifications.
Only one running Branchline process may own the Windows sender for the same app
identity; another process uses this fallback rather than taking over its clicks.
Initial history, stale/disconnected instances and unassigned sessions do not alert.
Repeated session/status alerts are suppressed for ten seconds; at most three
alerts are accepted per thirty seconds. The app retains at most 24 notifications.

This requires a supported ACP connection or explicitly attached OpenCode HTTP/SSE
observer. Plain terminal output is not inspected for questions or permissions.
For a read-only OpenCode observer, answer in OpenCode itself.

## Local verification

After building the Windows application and installing frontend dependencies:

```powershell
cargo build --manifest-path backend/Cargo.toml --release --locked --example acp-fixture --target-dir desktop/target
node desktop/tests/notifications.mjs --fallback
node desktop/tests/notifications.mjs
```

The second smoke requires clicking the real Windows notification. It records a
native-click pass only after receiving the actual activation and verifying the
selected worktree and session. These tests use temporary repositories, an offline
ACP fixture and separate configuration, without model calls.
