mod common;
use agent_workspace_backend::{
    git::{
        status::{parse_status, FileKind},
        GitCommand, GitRunner,
    },
    projects::{NewProject, Registry},
    worktrees::WorktreeService,
};
use common::{git, Repositories};
use std::{path::Path, process::Command};

fn commit(path: &Path, message: &str) {
    git(
        path,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

async fn status(path: &Path) -> agent_workspace_backend::git::status::GitStatus {
    let before = std::fs::read(path.join(".git/index")).unwrap();
    let bytes = GitRunner::default()
        .run(path, GitCommand::Status)
        .await
        .unwrap();
    assert_eq!(std::fs::read(path.join(".git/index")).unwrap(), before);
    parse_status(&bytes).unwrap()
}

#[tokio::test]
async fn real_staged_unstaged_rename_untracked_delete_binary_without_index_writes() {
    let repos = Repositories::new();
    let both = if cfg!(windows) {
        "both name"
    } else {
        "both name\n"
    };
    let renamed = if cfg!(windows) {
        "new имя"
    } else {
        "new имя\n"
    };
    let untracked = if cfg!(windows) {
        "untracked имя"
    } else {
        "untracked имя\n"
    };
    for file in [both, "old name", "delete", "binary"] {
        std::fs::write(repos.main.join(file), b"initial\0content\n").unwrap();
    }
    std::fs::write(repos.main.join(".gitignore"), "ignored\n").unwrap();
    git(&repos.main, &["add", "--all"]);
    commit(&repos.main, "tracked");
    std::fs::write(repos.main.join(both), "staged").unwrap();
    git(&repos.main, &["add", "--", both]);
    std::fs::write(repos.main.join(both), "unstaged").unwrap();
    std::fs::rename(repos.main.join("old name"), repos.main.join(renamed)).unwrap();
    git(&repos.main, &["add", "-A", "--", "old name", renamed]);
    std::fs::remove_file(repos.main.join("delete")).unwrap();
    std::fs::write(repos.main.join("binary"), b"changed\0binary").unwrap();
    std::fs::write(repos.main.join(untracked), "untracked").unwrap();
    std::fs::write(repos.main.join("ignored"), "ignored").unwrap();
    let status = status(&repos.main).await;
    assert_eq!(
        (
            status.staged,
            status.unstaged,
            status.untracked,
            status.conflicts
        ),
        (Some(2), Some(3), Some(1), Some(0))
    );
    let rename = status
        .files
        .iter()
        .find(|file| file.kind == FileKind::Renamed)
        .unwrap();
    assert_eq!(rename.original_path.as_deref(), Some(Path::new("old name")));
    assert_eq!(rename.path, Path::new(renamed));
    assert_eq!(
        status
            .files
            .iter()
            .find(|file| file.path == Path::new("delete"))
            .unwrap()
            .worktree_status,
        "D"
    );
    assert!(!status
        .files
        .iter()
        .any(|file| file.path == Path::new("ignored")));
    assert!(status.last_successful_refresh_at.is_some() && status.error.is_none());
}

#[tokio::test]
async fn real_merge_conflict_is_counted_separately_and_index_is_preserved() {
    let repos = Repositories::new();
    std::fs::write(repos.main.join("conflict"), "base\n").unwrap();
    git(&repos.main, &["add", "--", "conflict"]);
    commit(&repos.main, "base");
    git(&repos.main, &["switch", "-c", "side"]);
    std::fs::write(repos.main.join("conflict"), "side\n").unwrap();
    git(&repos.main, &["add", "--", "conflict"]);
    commit(&repos.main, "side");
    git(&repos.main, &["switch", "main"]);
    std::fs::write(repos.main.join("conflict"), "main\n").unwrap();
    git(&repos.main, &["add", "--", "conflict"]);
    commit(&repos.main, "main");
    let output = Command::new("git")
        .arg("-C")
        .arg(&repos.main)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "merge",
            "side",
            "--no-edit",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let status = status(&repos.main).await;
    assert_eq!(
        (status.staged, status.unstaged, status.conflicts),
        (Some(0), Some(0), Some(1))
    );
    assert_eq!(status.files[0].kind, FileKind::Unmerged);
    assert_eq!(status.files[0].index_status, "U");
}

#[tokio::test]
async fn real_submodule_dirty_flags_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init"]);
    let sub_repo = directory.path().join("sub-repo");
    std::fs::create_dir(&sub_repo).unwrap();
    git(&sub_repo, &["init"]);
    std::fs::write(sub_repo.join("tracked"), "initial").unwrap();
    git(&sub_repo, &["add", "--all"]);
    commit(&sub_repo, "initial");
    git(
        &main,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_repo.to_str().unwrap(),
            "sub",
        ],
    );
    commit(&main, "submodule");
    std::fs::write(main.join("sub/tracked"), "modified").unwrap();
    std::fs::write(main.join("sub/untracked"), "new").unwrap();
    let sub_index = std::fs::read(main.join(".git/modules/sub/index")).unwrap();
    let status = status(&main).await;
    assert_eq!(
        std::fs::read(main.join(".git/modules/sub/index")).unwrap(),
        sub_index
    );
    assert_eq!(status.unstaged, Some(1));
    let sub = status.files[0].submodule.as_ref().unwrap();
    assert!(sub.modified && sub.untracked && !sub.commit_changed);
}

#[tokio::test]
async fn unavailable_status_retains_last_success_with_error_and_bare_is_not_clean() {
    let repos = Repositories::new();
    let registry = Registry::open(repos.directory.path().join("config.json")).unwrap();
    let project = registry
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    std::fs::write(repos.linked.join("dirty"), "fixture").unwrap();
    let service = WorktreeService::new(registry.clone());
    let snapshot = service.refresh(project.id).await.unwrap();
    let linked = snapshot
        .worktrees
        .iter()
        .find(|tree| tree.path == repos.linked)
        .unwrap();
    assert_eq!(linked.git_status.untracked, Some(1));
    let last = linked.git_status.last_successful_refresh_at;
    let id = linked.id;
    std::fs::rename(&repos.linked, repos.directory.path().join("missing")).unwrap();
    let status = service.status(id).await.unwrap();
    assert_eq!(status.last_successful_refresh_at, last);
    assert_eq!(status.untracked, Some(1));
    assert!(status.error.is_some());
    let bare = repos.directory.path().join("bare.git");
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare"]);
    let project = registry
        .add(NewProject {
            root_path: bare.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let snapshot = service.refresh(project.id).await.unwrap();
    assert_eq!(snapshot.worktrees[0].git_status.staged, None);
    assert_eq!(
        snapshot.worktrees[0]
            .git_status
            .error
            .as_ref()
            .unwrap()
            .code,
        "bare_repository"
    );
}
