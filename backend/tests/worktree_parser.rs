mod common;
use agent_workspace_backend::worktrees::parser::parse_worktrees;
use common::{git, Repositories};
use std::{path::Path, process::Command};

fn snapshot(path: &Path) -> Vec<agent_workspace_backend::worktrees::parser::WorktreeRecord> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()
        .unwrap();
    assert!(output.status.success());
    parse_worktrees(&output.stdout).unwrap()
}

#[test]
fn real_git_main_linked_detached_locked_and_prunable() {
    let repos = Repositories::new();
    let detached = repos.directory.path().join("detached");
    git(
        &repos.main,
        &["worktree", "add", "--detach", detached.to_str().unwrap()],
    );
    git(
        &repos.main,
        &[
            "worktree",
            "lock",
            "--reason",
            "reason\nUnicode: причина",
            repos.linked.to_str().unwrap(),
        ],
    );
    let records = snapshot(&repos.linked);
    assert_eq!(records.len(), 3);
    assert_eq!(
        agent_workspace_backend::environment::canonical_path(&records[0].path).unwrap(),
        repos.main
    );
    assert_eq!(
        agent_workspace_backend::environment::canonical_path(&records[1].path).unwrap(),
        agent_workspace_backend::environment::canonical_path(&detached).unwrap()
    );
    assert!(records[1].is_detached);
    let locked = records
        .iter()
        .find(|record| {
            agent_workspace_backend::environment::canonical_path(&record.path)
                .ok()
                .as_ref()
                == Some(&repos.linked)
        })
        .unwrap();
    assert_eq!(
        locked.lock_reason.as_deref(),
        Some("reason\nUnicode: причина")
    );
    std::fs::rename(&detached, repos.directory.path().join("moved")).unwrap();
    assert!(
        snapshot(&repos.main)
            .iter()
            .find(|record| record.path == detached)
            .unwrap()
            .is_prunable
    );
}

#[test]
fn real_git_bare_and_unborn() {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "--bare"]);
    assert!(snapshot(directory.path())[0].is_bare);
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    let record = snapshot(directory.path()).remove(0);
    assert_eq!(record.head.unwrap(), "0".repeat(40));
    assert_eq!(record.branch.as_deref(), Some("main"));
}
