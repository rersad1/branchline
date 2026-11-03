//! Read-only inventory. Only Java argv is read locally by a bounded role classifier;
//! argv, environment, cwd, cgroup paths and terminal output are never serialized.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[cfg(unix)]
mod linux;
#[cfg(windows)]
mod windows;

pub const MAX_PROCESSES: usize = 128;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Pressure {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_us: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SystemMemory {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub cached_bytes: Option<u64>,
    pub reclaimable_slab_bytes: Option<u64>,
    pub unreclaimable_slab_bytes: Option<u64>,
    pub anonymous_bytes: Option<u64>,
    pub shmem_bytes: Option<u64>,
    pub swap_total_bytes: Option<u64>,
    pub swap_free_bytes: Option<u64>,
    #[serde(default)]
    pub pressure_some: Option<Pressure>,
    #[serde(default)]
    pub pressure_full: Option<Pressure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalMemory {
    pub session_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub supervisor_pid: Option<u32>,
    pub state: String,
    pub buffer_bytes: usize,
    pub buffer_capacity_bytes: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessMemory {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub category: String,
    pub relation: String,
    pub resident_bytes: Option<u64>,
    /// Linux USS; Windows private commit (NOT resident private working set).
    pub private_bytes: Option<u64>,
    pub pss_bytes: Option<u64>,
    pub swap_bytes: Option<u64>,
    pub threads: u32,
    pub cpu_time_ms: Option<u64>,
    /// Linux /proc start ticks; Windows creation FILETIME. Interpret per platform.
    pub start_id: Option<u64>,
    pub session_id: Option<Uuid>,
    pub worktree_id: Option<Uuid>,
    pub metrics_error: Option<String>,
    #[serde(default)]
    pub lifetime_id: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub classification: String,
    #[serde(default)]
    pub container: Option<bool>,
    #[serde(default)]
    pub resident_delta_bytes: Option<i64>,
    #[serde(default)]
    pub pss_delta_bytes: Option<i64>,
    #[serde(default)]
    pub cpu_delta_ms: Option<u64>,
    #[serde(default)]
    pub cpu_percent: Option<f64>,
    #[serde(default)]
    pub peak_resident_bytes: Option<u64>,
    #[serde(default)]
    pub peak_pss_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Totals {
    pub processes: usize,
    pub resident_bytes: u64,
    pub pss_bytes: u64,
    pub swap_bytes: u64,
    pub threads: u64,
    pub cpu_time_ms: u64,
    pub missing_rss: usize,
    pub missing_pss: usize,
    pub missing_swap: usize,
    pub missing_cpu: usize,
    pub missing_start: usize,
    /// Per-process PSS where available, RSS otherwise. Missing both is unknown.
    pub accounted_bytes: u64,
    pub missing_accounting: usize,
}
impl Totals {
    fn add(&mut self, p: &ProcessMemory) {
        self.processes += 1;
        self.resident_bytes += p.resident_bytes.unwrap_or(0);
        self.pss_bytes += p.pss_bytes.unwrap_or(0);
        self.swap_bytes += p.swap_bytes.unwrap_or(0);
        self.threads += u64::from(p.threads);
        self.cpu_time_ms += p.cpu_time_ms.unwrap_or(0);
        self.missing_rss += usize::from(p.resident_bytes.is_none());
        self.missing_pss += usize::from(p.pss_bytes.is_none());
        self.missing_swap += usize::from(p.swap_bytes.is_none());
        self.missing_cpu += usize::from(p.cpu_time_ms.is_none());
        self.missing_start += usize::from(p.start_id.is_none());
        self.accounted_bytes += p.pss_bytes.or(p.resident_bytes).unwrap_or(0);
        self.missing_accounting += usize::from(p.pss_bytes.or(p.resident_bytes).is_none());
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CategoryTotal {
    pub category: String,
    pub totals: Totals,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GroupSummary {
    pub scope: String,
    pub worktree_id: Option<Uuid>,
    pub totals: Totals,
    pub categories: Vec<CategoryTotal>,
    pub peak_accounted_bytes: u64,
    pub accounted_delta_bytes: Option<i64>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Coverage {
    pub discovered_processes: usize,
    pub unreadable_processes: usize,
    pub excluded_processes: usize,
    pub detailed_processes: usize,
    pub inventory_complete: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Snapshot {
    pub platform: String,
    pub collector_pid: u32,
    pub system: SystemMemory,
    pub processes: Vec<ProcessMemory>,
    pub terminals: Vec<TerminalMemory>,
    pub scanned_processes: usize,
    pub omitted_processes: usize,
    pub pss_readable_processes: usize,
    pub errors: Vec<String>,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub groups: Vec<GroupSummary>,
    #[serde(default)]
    pub coverage: Coverage,
}

fn source_id() -> String {
    // A worker restart deliberately starts a new observation lifetime, even within
    // the same Linux boot. No machine identifiers or filesystem paths are exported.
    static RUN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUN.get_or_init(|| {
        #[cfg(unix)]
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .and_then(|s| Uuid::parse_str(s.trim()).ok())
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown-boot".into());
        #[cfg(windows)]
        let boot = "windows";
        format!("{boot}:{}", Uuid::new_v4())
    })
    .clone()
}

pub fn sample(terminals: &crate::terminal::TerminalService) -> Snapshot {
    let terminals = terminals.memory_sessions();
    #[cfg(unix)]
    return linux::sample(terminals);
    #[cfg(windows)]
    return windows::sample(terminals);
}

fn category(name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    if name.contains("vmmem") {
        "wsl_vm"
    } else if name.contains("opencode") {
        "opencode"
    } else if name.contains("webview") {
        "webview"
    } else if name == "java" || name == "java.exe" || name == "javaw.exe" {
        "java"
    } else if name.contains("mcp") {
        "mcp"
    } else if matches!(name.as_str(), "node" | "node.exe" | "bun" | "bun.exe") {
        "node_bun"
    } else if ["docker", "containerd", "dockerd", "podman"]
        .iter()
        .any(|n| name.starts_with(n))
    {
        "container_service"
    } else if name.contains("terminal-host") {
        "pty_supervisor"
    } else if [
        "rust-analyzer",
        "tsserver",
        "language-server",
        "clangd",
        "pyright",
    ]
    .iter()
    .any(|n| name.contains(n))
    {
        "language_server"
    } else if ["bash", "powershell", "pwsh", "zsh", "fish"]
        .iter()
        .any(|n| name.starts_with(n))
    {
        "shell"
    } else {
        "other"
    }
}

fn bounded_name(name: &str) -> String {
    if name.contains(['/', '\\']) {
        return "unknown_process".into();
    }
    name.chars().filter(|c| !c.is_control()).take(64).collect()
}

fn attribute(
    mut processes: Vec<ProcessMemory>,
    terminals: &[TerminalMemory],
    collector: u32,
    linux: bool,
) -> (Vec<ProcessMemory>, usize) {
    let count_before = processes.len();
    let parents: HashMap<_, _> = processes.iter().map(|p| (p.pid, p.parent_pid)).collect();
    let starts: HashMap<_, _> = processes
        .iter()
        .filter_map(|p| p.start_id.map(|s| (p.pid, s)))
        .collect();
    let names: HashMap<_, _> = processes.iter().map(|p| (p.pid, p.name.clone())).collect();
    let mut roots: HashMap<u32, Option<&TerminalMemory>> = HashMap::new();
    for t in terminals.iter().filter(|t| t.state == "running") {
        if let Some(pid) = t.supervisor_pid {
            roots
                .entry(pid)
                .and_modify(|owner| *owner = None)
                .or_insert(Some(t));
        }
    }
    for p in &mut processes {
        p.session_id = None;
        p.worktree_id = None;
        p.name = bounded_name(&p.name);
        p.category = category(&p.name).into();
        p.classification = "name_heuristic".into();
        p.relation = "other_process".into();
        let mut pid = p.pid;
        let mut seen = HashSet::new();
        while pid != 0 && seen.insert(pid) {
            if roots.get(&pid).is_some_and(|owner| owner.is_none()) {
                p.relation = "ambiguous_terminal_ancestry".into();
                break;
            }
            if let Some(t) = roots
                .get(&pid)
                .and_then(|t| *t)
                .filter(|_| starts.contains_key(&pid))
            {
                p.session_id = Some(t.session_id);
                p.worktree_id = Some(t.worktree_id);
                p.relation = "terminal_descendant".into();
                break;
            }
            if pid == collector {
                p.relation = "app_descendant".into();
                break;
            }
            let parent = *parents.get(&pid).unwrap_or(&0);
            if p.category == "node_bun"
                && p.role.is_empty()
                && names
                    .get(&parent)
                    .is_some_and(|n| category(n) == "opencode")
            {
                p.role = "opencode_child_runtime".into();
                p.classification = "name_and_parent_heuristic".into();
            }
            // A recycled parent PID cannot be an ancestor of an older child.
            if starts
                .get(&parent)
                .zip(starts.get(&pid))
                .is_none_or(|(a, b)| a > b)
            {
                break;
            }
            pid = parent;
        }
        if p.pid == collector {
            p.category = "app".into();
        }
        if p.category == "java" {
            p.classification = if p.role.is_empty() {
                p.role = "unknown_jvm".into();
                "name_heuristic"
            } else {
                "bounded_java_role_heuristic"
            }
            .into();
        }
    }
    // Keep all app/PTY descendants first, then the largest other Linux processes.
    // Windows includes only this app tree, WSL VMs and named OpenCode processes.
    processes.retain(|p| {
        linux
            || p.relation != "other_process"
            || matches!(p.category.as_str(), "wsl_vm" | "opencode")
    });
    let excluded = count_before.saturating_sub(processes.len());
    (processes, excluded)
}

#[cfg(test)]
fn select(
    mut processes: Vec<ProcessMemory>,
    terminals: &[TerminalMemory],
    collector: u32,
    linux: bool,
) -> (Vec<ProcessMemory>, usize) {
    processes = attribute(processes, terminals, collector, linux).0;
    let count = processes.len();
    prioritize(&mut processes);
    processes.truncate(MAX_PROCESSES);
    (processes, count.saturating_sub(MAX_PROCESSES))
}

fn prioritize(processes: &mut [ProcessMemory]) {
    processes.sort_by_key(|p| {
        (
            std::cmp::Reverse(p.relation != "other_process" || p.category == "wsl_vm"),
            std::cmp::Reverse(p.resident_bytes.unwrap_or(0)),
            p.pid,
        )
    });
}

fn finish(mut snapshot: Snapshot, mut all: Vec<ProcessMemory>) -> Snapshot {
    snapshot.source_id = source_id();
    let mut groups: std::collections::BTreeMap<(String, Option<Uuid>), GroupSummary> =
        Default::default();
    for p in &mut all {
        p.lifetime_id = p
            .start_id
            .map(|s| format!("{}:{}:{s}", snapshot.source_id, p.pid));
        let scope = if p.worktree_id.is_some() {
            "worktree"
        } else if p.relation == "app_descendant"
            || matches!(p.category.as_str(), "wsl_vm" | "container_service")
        {
            "shared_service"
        } else {
            "unattributed"
        };
        let key = (scope.to_string(), p.worktree_id);
        let group = groups.entry(key).or_insert_with(|| GroupSummary {
            scope: scope.into(),
            worktree_id: p.worktree_id,
            ..Default::default()
        });
        group.totals.add(p);
        if let Some(c) = group
            .categories
            .iter_mut()
            .find(|c| c.category == p.category)
        {
            c.totals.add(p);
        } else {
            let mut c = CategoryTotal {
                category: p.category.clone(),
                ..Default::default()
            };
            c.totals.add(p);
            group.categories.push(c);
        }
    }
    snapshot.groups = groups.into_values().collect();
    for g in &mut snapshot.groups {
        g.categories.sort_by(|a, b| a.category.cmp(&b.category));
    }
    snapshot.omitted_processes = all.len().saturating_sub(MAX_PROCESSES);
    snapshot.pss_readable_processes = all.iter().filter(|p| p.pss_bytes.is_some()).count();
    prioritize(&mut all);
    all.truncate(MAX_PROCESSES);
    snapshot.coverage.detailed_processes = all.len();
    snapshot.processes = all;
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_before_truncation_without_double_counting_and_preserves_unknowns() {
        let tree = Uuid::new_v4();
        let all: Vec<_> = (1..=200)
            .map(|pid| ProcessMemory {
                pid,
                worktree_id: Some(tree),
                category: "java".into(),
                resident_bytes: Some(100),
                pss_bytes: (pid == 1).then_some(50),
                threads: 2,
                ..Default::default()
            })
            .collect();
        let snapshot = finish(Snapshot::default(), all);
        assert_eq!(snapshot.processes.len(), 128);
        assert_eq!(snapshot.omitted_processes, 72);
        assert_eq!(snapshot.groups.len(), 1);
        let g = &snapshot.groups[0];
        assert_eq!(g.totals.processes, 200);
        assert_eq!(g.totals.accounted_bytes, 19950);
        assert_eq!(g.totals.pss_bytes, 50);
        assert_eq!(g.totals.missing_pss, 199);
        assert_eq!(g.totals.missing_swap, 200);
        assert_eq!(g.totals.threads, 400);
        assert_eq!(g.categories[0].totals.processes, 200);
    }
    #[test]
    fn unknown_and_shared_processes_stay_outside_worktree_and_reused_parent_is_rejected() {
        let t = TerminalMemory {
            session_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            supervisor_pid: Some(10),
            state: "running".into(),
            buffer_bytes: 0,
            buffer_capacity_bytes: 0,
        };
        let rows = vec![
            ProcessMemory {
                pid: 1,
                start_id: Some(1),
                ..Default::default()
            },
            ProcessMemory {
                pid: 10,
                parent_pid: 1,
                start_id: Some(100),
                ..Default::default()
            },
            ProcessMemory {
                pid: 20,
                parent_pid: 10,
                start_id: Some(50),
                ..Default::default()
            },
            ProcessMemory {
                pid: 30,
                parent_pid: 10,
                start_id: Some(120),
                ..Default::default()
            },
        ];
        let (rows, _) = attribute(rows, std::slice::from_ref(&t), 1, true);
        assert!(rows[2].worktree_id.is_none());
        assert_eq!(rows[3].worktree_id, Some(t.worktree_id));
        let snapshot = finish(Snapshot::default(), rows);
        assert_eq!(
            snapshot
                .groups
                .iter()
                .map(|g| g.totals.processes)
                .sum::<usize>(),
            4
        );
        assert!(snapshot.groups.iter().any(|g| g.scope == "shared_service"));
        assert!(snapshot.groups.iter().any(|g| g.scope == "unattributed"));
    }
    #[test]
    fn conflicting_terminal_roots_never_pick_an_arbitrary_owner() {
        let a = TerminalMemory {
            session_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            supervisor_pid: Some(10),
            state: "running".into(),
            buffer_bytes: 0,
            buffer_capacity_bytes: 0,
        };
        let b = TerminalMemory {
            session_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            ..a.clone()
        };
        let rows = vec![ProcessMemory {
            pid: 10,
            start_id: Some(1),
            ..Default::default()
        }];
        let (rows, _) = attribute(rows, &[a, b], 1, false);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].worktree_id.is_none());
        assert_eq!(rows[0].relation, "ambiguous_terminal_ancestry");
    }
    #[test]
    fn legacy_snapshot_and_fixed_categories_remain_readable() {
        let old = r#"{"platform":"linux","collector_pid":1,"system":{},"processes":[{"pid":1,"parent_pid":0,"name":"java","category":"other","relation":"other_process","threads":1}],"terminals":[],"scanned_processes":1,"omitted_processes":0,"pss_readable_processes":0,"errors":[]}"#;
        let snapshot: Snapshot = serde_json::from_str(old).unwrap();
        assert!(snapshot.groups.is_empty());
        assert!(snapshot.processes[0].lifetime_id.is_none());
        assert!(snapshot.processes[0].role.is_empty());
        assert!(!snapshot.coverage.inventory_complete);
        assert_eq!(category("java.exe"), "java");
        assert_eq!(category("mcp-atlassian"), "mcp");
        assert_eq!(category("bun"), "node_bun");
        assert_eq!(category("containerd-shim"), "container_service");
    }
    #[test]
    fn attribution_survives_nested_children_and_cycles_and_limits_inventory() {
        let terminal = TerminalMemory {
            session_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            supervisor_pid: Some(10),
            state: "running".into(),
            buffer_bytes: 0,
            buffer_capacity_bytes: 0,
        };
        let mut rows: Vec<_> = (1..200)
            .map(|pid| ProcessMemory {
                pid,
                parent_pid: 0,
                name: "other\n".into(),
                resident_bytes: Some(1000),
                start_id: Some(u64::from(pid)),
                ..Default::default()
            })
            .collect();
        rows[9].parent_pid = 5;
        rows[10].parent_pid = 10;
        rows[11].parent_pid = 11;
        rows[19].parent_pid = 21;
        rows[20].parent_pid = 20;
        let (selected, omitted) = select(rows, std::slice::from_ref(&terminal), 5, true);
        assert_eq!(selected.len(), MAX_PROCESSES);
        assert_eq!(omitted, 199 - MAX_PROCESSES);
        assert_eq!(
            selected.iter().find(|p| p.pid == 12).unwrap().worktree_id,
            Some(terminal.worktree_id)
        );
        assert!(selected.iter().all(|p| !p.name.contains('\n')));
    }
    #[test]
    fn native_sample_has_collector_and_no_paths_or_arguments() {
        let snapshot = sample(&Default::default());
        assert!(snapshot.system.total_bytes.unwrap_or(0) > 0);
        assert!(snapshot
            .processes
            .iter()
            .any(|p| p.pid == std::process::id() && p.resident_bytes.unwrap_or(0) > 0));
        assert!(snapshot.processes.len() <= MAX_PROCESSES);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("cmdline"));
        assert!(!json.contains("AWM_TOKEN"));
    }
    #[test]
    fn reports_native_collection_cost_without_exporting_local_data() {
        let start = std::time::Instant::now();
        let snapshot = sample(&Default::default());
        let bytes = serde_json::to_vec(&snapshot).unwrap().len();
        println!("memory benchmark: platform={} elapsed_ms={} snapshot_bytes={} discovered={} detailed={} groups={} readable_pss={}",snapshot.platform,start.elapsed().as_millis(),bytes,snapshot.coverage.discovered_processes,snapshot.processes.len(),snapshot.groups.len(),snapshot.pss_readable_processes);
    }
}
