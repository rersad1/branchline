//! Read-only, bounded memory inventory. Never reads argv, environment, cwd or terminal output.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[cfg(unix)]
mod linux;
#[cfg(windows)]
mod windows;

pub const MAX_PROCESSES: usize = 128;

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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    name.chars().filter(|c| !c.is_control()).take(64).collect()
}

fn select(
    mut processes: Vec<ProcessMemory>,
    terminals: &[TerminalMemory],
    collector: u32,
    linux: bool,
) -> (Vec<ProcessMemory>, usize) {
    let parents: HashMap<_, _> = processes.iter().map(|p| (p.pid, p.parent_pid)).collect();
    let roots: HashMap<_, _> = terminals
        .iter()
        .filter(|t| t.state == "running")
        .filter_map(|t| t.supervisor_pid.map(|pid| (pid, t)))
        .collect();
    for p in &mut processes {
        p.name = bounded_name(&p.name);
        p.category = category(&p.name).into();
        p.relation = "other_process".into();
        let mut pid = p.pid;
        let mut seen = HashSet::new();
        while pid != 0 && seen.insert(pid) {
            if let Some(t) = roots.get(&pid) {
                p.session_id = Some(t.session_id);
                p.worktree_id = Some(t.worktree_id);
                p.relation = "terminal_descendant".into();
                break;
            }
            if pid == collector {
                p.relation = "app_descendant".into();
                break;
            }
            pid = *parents.get(&pid).unwrap_or(&0);
        }
        if p.pid == collector {
            p.category = "app".into();
        }
    }
    // Keep all app/PTY descendants first, then the largest other Linux processes.
    // Windows includes only this app tree, WSL VMs and named OpenCode processes.
    processes.retain(|p| {
        linux
            || p.relation != "other_process"
            || matches!(p.category.as_str(), "wsl_vm" | "opencode")
    });
    let count = processes.len();
    processes.sort_by_key(|p| {
        (
            std::cmp::Reverse(p.relation != "other_process" || p.category == "wsl_vm"),
            std::cmp::Reverse(p.resident_bytes.unwrap_or(0)),
            p.pid,
        )
    });
    processes.truncate(MAX_PROCESSES);
    (processes, count.saturating_sub(MAX_PROCESSES))
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
