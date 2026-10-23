mod common;
use agent_workspace_backend::{
    projects::{NewProject, Registry},
    remove_worktree::{self, Request},
    service::WorkspaceService,
};
use common::{git, Repositories};

#[tokio::test]
async fn removes_only_confirmed_missing_linked_entry_and_preserves_branches_and_index() {
    let repos = Repositories::new();
    let service =
        WorkspaceService::new(Registry::open(repos.directory.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap()
        .project;
    let snapshot = service.worktrees.refresh(project.id).await.unwrap();
    let tree = snapshot.worktrees.iter().find(|t| !t.is_main).unwrap();
    let mut request = Request {
        project_id: project.id,
        worktree_id: tree.id,
        confirm_path: tree.path.to_str().unwrap().into(),
        external_sessions_checked: true,
        missing_only: true,
    };
    let index = std::fs::read(repos.main.join(".git/index")).ok();
    // Even a clean available directory must never be accepted by missing-only mode.
    assert_eq!(
        remove_worktree::remove(&service, request.clone())
            .await
            .unwrap_err()
            .0,
        "remove_scope"
    );
    let other = repos.main.parent().unwrap().join("other missing");
    git(
        &repos.main,
        &[
            "worktree",
            "add",
            "-b",
            "other-missing",
            other.to_str().unwrap(),
        ],
    );
    assert_eq!(repos.linked.parent(), other.parent());
    std::fs::remove_dir_all(&repos.linked).unwrap();
    std::fs::remove_dir_all(&other).unwrap();
    request.missing_only = false;
    assert_eq!(
        remove_worktree::remove(&service, request.clone())
            .await
            .unwrap_err()
            .0,
        "remove_scope"
    );
    request.missing_only = true;
    request.external_sessions_checked = false;
    assert_eq!(
        remove_worktree::remove(&service, request.clone())
            .await
            .unwrap_err()
            .0,
        "remove_confirmation"
    );
    request.external_sessions_checked = true;
    // A folder that reappeared before acceptance is not discarded, even with a stale UI.
    std::fs::create_dir(&repos.linked).unwrap();
    std::fs::write(repos.linked.join("keep"), "user data").unwrap();
    assert_eq!(
        remove_worktree::remove(&service, request.clone())
            .await
            .unwrap_err()
            .0,
        "remove_scope"
    );
    assert_eq!(
        std::fs::read_to_string(repos.linked.join("keep")).unwrap(),
        "user data"
    );
    std::fs::remove_file(repos.linked.join("keep")).unwrap();
    std::fs::remove_dir(&repos.linked).unwrap();
    // Exact path confirmation is required for missing registrations too.
    let mut wrong = request.clone();
    wrong.confirm_path = other.to_str().unwrap().into();
    assert_eq!(
        remove_worktree::remove(&service, wrong)
            .await
            .unwrap_err()
            .0,
        "remove_confirmation"
    );
    let result = remove_worktree::remove(&service, request).await.unwrap();
    assert!(result.git_removed);
    assert_eq!(result.path_exists, Some(false));
    let remaining = result.snapshot.unwrap();
    assert_eq!(remaining.worktrees.len(), 2);
    assert!(remaining
        .worktrees
        .iter()
        .any(|t| t.path == other && t.is_prunable));
    git(&repos.main, &["show-ref", "--verify", "refs/heads/linked"]);
    git(
        &repos.main,
        &["show-ref", "--verify", "refs/heads/other-missing"],
    );
    assert_eq!(std::fs::read(repos.main.join(".git/index")).ok(), index);
}

#[tokio::test]
async fn missing_locked_entry_is_protected() {
    let repos = Repositories::new();
    let service =
        WorkspaceService::new(Registry::open(repos.directory.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap()
        .project;
    let snapshot = service.worktrees.refresh(project.id).await.unwrap();
    let tree = snapshot.worktrees.iter().find(|t| !t.is_main).unwrap();
    git(
        &repos.main,
        &["worktree", "lock", repos.linked.to_str().unwrap()],
    );
    assert_eq!(repos.linked.parent(), repos.main.parent());
    std::fs::remove_dir_all(&repos.linked).unwrap();
    let request = Request {
        project_id: project.id,
        worktree_id: tree.id,
        confirm_path: tree.path.to_str().unwrap().into(),
        external_sessions_checked: true,
        missing_only: true,
    };
    assert_eq!(
        remove_worktree::remove(&service, request)
            .await
            .unwrap_err()
            .0,
        "remove_protected"
    );
    assert_eq!(
        service
            .worktrees
            .refresh(project.id)
            .await
            .unwrap()
            .worktrees
            .len(),
        2
    );
}
