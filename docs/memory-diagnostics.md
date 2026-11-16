# Memory diagnostics and worktree budgets

Open **Диагностика памяти** from the toolbar or command palette. Recording is opt-in
and retains its setting across restarts. It runs only while Branchline is open,
on the existing 30-second cycle. Only connected WSL workers are sampled; monitoring
does not start or reconnect a distro.

## Enable a soft budget

In **Мягкий бюджет worktree**, select the environment and worktree, enter a value in
MiB or GiB, and press **Сохранить бюджет**. Units are binary (1 GiB = 1024 MiB).
Budgets are disabled by default. **Выключить бюджет** persists the disabled setting.
Settings are keyed by both environment UUID and worktree UUID, stored in
`budgets.json` beside the local diagnostics logs, outside the source checkout by
default. Relinking an environment does not silently transfer a budget to another
environment identity. No cloud synchronization is performed.

This is a **warning budget, not a hard RAM limit**. It does not change OS/WSL
settings, stop or suspend processes, pause OpenCode, or block commands. An exceeded
budget produces a persistent in-app notice and one `budget_exceeded` log event.
Repeats are suppressed until a complete observation is at or below 90% of the
budget; then `budget_recovered` rearms the warning. Editing a budget resets that
budget's suppression. Suppression and peaks belong to the current app session.
Settings take effect on the next existing collection cycle.

The UI explicitly reports disabled diagnostics, stale/awaited observations,
unavailable environments or ownership, and incomplete accounting. A saved budget
does not enable recording. A missing environment or missing owned group is unknown,
not zero consumption. Short peaks between observations can be missed.

## What is counted

Each observed process belongs to at most one group: a worktree, shared services,
or unattributed processes. Ownership follows the nearest running registered PTY
supervisor through observed parent links. Parent creation/start times are checked
to reject recycled PIDs. No worktree is inferred from cwd or a project path.
Shared app services, Vmmem and container engines do not get assigned to an arbitrary
worktree. Orphans, external shells, separately launched containers, hidden PID
namespaces and shared MCP servers may remain outside a worktree's budget. Terminal
buffer sizes are separate metadata and are not added to process memory.
Conflicting registered supervisor roots remain unattributed.

The budget metric is **sum of per-process PSS, falling back to RSS for each process
whose PSS is unavailable**. This also works on Windows, where PSS is unavailable.
RSS includes shared pages, so fallback can conservatively overcount them. If both
metrics are unavailable for any owned process, or inventory is incomplete, a below
budget value cannot establish compliance. Known consumption above the budget still
warns even with incomplete accounting. The UI exposes fallback and missing counts.
The metric measures resident memory and excludes swap; it is an observation of the
attributed population, not a bound on all activity caused by a project.

Lightweight RSS, swap, threads and CPU totals are aggregated **before** limiting
details. Expensive Linux `smaps_rollup` reads and detailed process rows are limited
to 128, prioritizing app/terminal descendants and then large processes. Windows
creation times and a read-only system working-set table support lightweight totals;
unrelated Windows applications are outside this collector's scope. Linux inventory
covers the visible `/proc` namespace, including visible container processes.

Available sums are subtotals. Every group/category includes process and thread
counts, RSS, available PSS, swap, CPU, accounted bytes, and counts missing RSS/PSS,
swap/CPU/start/accounting. Inventory coverage includes discovered, unreadable,
excluded and detailed process counts. A process vanishing during collection counts
as unavailable rather than a measured zero. Kernel threads often have no resident
or PSS metrics. Unknown metrics are never silently treated as measured zeros.

Linux PSI `some` and `full`, when available, include 10/60/300-second stall
percentages and cumulative stall microseconds. Linux system memory/PSI can cover
multiple WSL2 distros. **Never add Windows Vmmem memory to the Linux process sums**;
these are different views of overlapping memory. Linux `private_bytes` means USS;
Windows `private_bytes` means private commit, including nonresident pages.

Process identity is environment + source (Linux boot ID and collector run UUID) +
PID + exact start ID. String `lifetime_id` avoids JavaScript precision loss with
Windows FILETIME. New collector runs, boot changes, missing observations, invalid
time ordering and gaps longer than 90 seconds break CPU/memory deltas. CPU percent
is elapsed CPU time divided by elapsed wall time and can exceed 100% on multiple
cores. Group memory deltas also reflect population and PSS/fallback changes.
Stopping and resuming recording also breaks deltas, even for a short pause.

Session peaks are observed per process (RSS/PSS) and per group (accounted bytes).
Different peaks may occur at different times and must not be added. History is
bounded to 2048 process lifetimes and 512 groups; capacity loss is explicit in
sample errors. At most 1024 events are retained per sample, with an omitted count.
Process events cover the bounded detail sample: `process_observed`,
`process_unobserved`, and `owner_changed`. **Disappearance does not prove exit**:
selection, permissions or visibility can change. Source changes, observation gaps,
environment unavailability and budget transitions have separate events.

## Classification and privacy

Names distinguish Java, OpenCode, MCP, Node/Bun, language servers, shells, PTY
supervisors, container services, app processes and WSL VMs. Parent names can mark a
Node/Bun runtime as an OpenCode child. These classifications are heuristics;
unrecognized Node/Python MCP servers cannot reliably be distinguished by name alone.
Linux cgroup text supplies only an optional heuristic container boolean; hidden or
unusual cgroups can prevent detection. No container IDs or cgroup paths are stored.

Only for Java in Linux, the collector reads at most 16 KiB of local argv to choose
one fixed role: `test_worker`, `gradle_daemon`, `build_tool`, `language_server`,
`compiler_daemon`, or `unknown_jvm`. It immediately discards the input; no raw tokens
or classifier input appear in errors, logs or export. Windows JVM role can remain
unknown. Name/parent matches and Java roles are explicitly marked as heuristics.

No full commands, arguments, environment variables, secrets, project paths,
terminal contents or prompts are saved. The export contains process basenames,
numeric metrics, fixed classifications, process/session/worktree/environment IDs,
Linux boot/run identity, budget settings and events. The diagnostics directory and
manual export path are displayed locally in the UI. Handle exports as private
diagnostic information. Do not commit local reports or settings; ignore patterns
also cover accidental report/config copies into the repository.

## Storage and export compatibility

Choose **Экспортировать сводку** for initial analysis. The separate JSON export
streams retained schema-1/schema-2 logs and reports observed min/max with timestamps,
per-environment category totals, app-session ranges, low Windows available-memory
counts and missing/incomplete observations. Category totals are combined across
groups in the same snapshot before measuring peaks. Old records without group totals
are explicitly marked as detail-only subtotals. Missing metrics are not measured
zeros; a range with `observations: 0` is unavailable. Counts describe samples, not
elapsed time across gaps. Sources are aggregated by environment across boots.

Summary memory is bounded to one input line (4 MiB), 64 sources, 128 app sessions,
32 categories per source and eight selected processes at each source's minimum
available-memory observation. Invalid/oversized lines and bounded entry loss are
counted. The summary is intentionally lossy and contains no process commands.
**Экспортировать полный журнал** preserves the existing streamed JSONL export.
Neither export changes the retained logs or recording state.

In a terminal workspace, **Освободить память…** offers an explicit stop of its
app-owned terminal/agent sessions, including their owned MCP descendants. Confirming
uses the existing scoped supervisor cleanup; it does not stop a distro, external
processes or another worktree. Tabs, order, split and launch descriptors remain;
**Запустить заново** explicitly starts a new session. There is no inactivity timer
or automatic termination of background tasks. Running jobs and unsaved state inside
the stopped processes can be lost. Partial failures leave remaining sessions visible.

Schema **2** adds `source_id`, `lifetime_id`, `coverage`, group/category totals,
pressure, deltas/peaks, events, session identity, snapshot size, budget settings and
statuses. Export begins with metadata describing units, privacy, limitations and
current budgets. Existing schema-1 JSONL records are streamed unchanged; new fields
absent from old data mean unavailable. Old process/snapshot fields remain readable.

Two rotating JSONL logs retain at most 64 MiB each (128 MiB combined). Partial final
lines are repaired after interrupted writes. Export uses streamed file copying;
the daily log is never loaded into RAM. Memory holds the latest bounded sample and
bounded tracking state. Manual exports remain until removed by the user.

## Checks

`cargo test --manifest-path backend/Cargo.toml memory:: --lib -- --nocapture`
includes a native collection benchmark reporting only elapsed milliseconds, byte
size and counts. Run it on Windows and inside the selected real Ubuntu WSL.
`cargo test --manifest-path desktop/Cargo.toml memory:: --lib` checks lifetime/gap
handling, ownership changes, partial accounting, environment scoping, budget
persistence/disable/exceed/recover/suppression, rotation and mixed-version streaming
export. Frontend diagnostics tests cover binary units, environment scoping,
inactive control and incomplete accounting. Full repository checks are listed in
the root README. These fixtures do not establish a sustained high-load or real
container/JVM acceptance run.
