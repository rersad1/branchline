mod common;
use agent_workspace_backend::{
    projects::{NewProject, Registry},
    worktrees::WorktreeService,
};
use common::{git, Repositories};

async fn registered(repos: &Repositories) -> (Registry, uuid::Uuid) {
    let registry = Registry::open(repos.directory.path().join("config.json")).unwrap();
    let project = registry
        .add(NewProject {
            root_path: repos.linked.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    (registry, project.id)
}

#[tokio::test]
async fn external_add_remove_and_branch_head_changes_keep_existing_ids() {
    let repos = Repositories::new();
    let (registry, project) = registered(&repos).await;
    let service = WorktreeService::new(registry);
    let initial = service.list(project).await.unwrap();
    assert!(initial.error.is_none());
    assert_eq!(initial.worktrees.len(), 2);
    let main = initial.worktrees.iter().find(|tree| tree.is_main).unwrap();
    assert_eq!(main.path, repos.main);
    assert!(main.is_available);
    let main_id = main.id;
    git(&repos.main, &["switch", "-c", "other"]);
    git(
        &repos.main,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "second",
        ],
    );
    let external = repos.directory.path().join("external");
    git(
        &repos.main,
        &[
            "worktree",
            "add",
            "-b",
            "external",
            external.to_str().unwrap(),
        ],
    );
    let updated = service.refresh(project).await.unwrap();
    assert_eq!(updated.worktrees.len(), 3);
    let main = updated.worktrees.iter().find(|tree| tree.is_main).unwrap();
    assert_eq!(main.id, main_id);
    assert_eq!(main.branch.as_deref(), Some("other"));
    let external_id = updated
        .worktrees
        .iter()
        .find(|tree| {
            tree.path == agent_workspace_backend::environment::canonical_path(&external).unwrap()
        })
        .unwrap()
        .id;
    git(
        &repos.main,
        &["worktree", "remove", external.to_str().unwrap()],
    );
    assert_eq!(service.refresh(project).await.unwrap().worktrees.len(), 2);
    assert!(service.get(external_id).await.is_err());
}

#[tokio::test]
async fn moved_missing_and_foreign_replacement_do_not_reuse_binding() {
    let repos = Repositories::new();
    let (registry, project) = registered(&repos).await;
    let service = WorktreeService::new(registry);
    let initial = service.refresh(project).await.unwrap();
    let linked_id = initial
        .worktrees
        .iter()
        .find(|tree| tree.path == repos.linked)
        .unwrap()
        .id;
    let moved = repos.directory.path().join("moved");
    std::fs::rename(&repos.linked, &moved).unwrap();
    let missing = service.get(linked_id).await.unwrap();
    assert!(!missing.is_available && missing.is_prunable);
    std::fs::create_dir(&repos.linked).unwrap();
    git(&repos.linked, &["init"]);
    assert!(!service.get(linked_id).await.unwrap().is_available);
    // Only fixture directories are removed; restore the original worktree before Git move.
    std::fs::remove_dir_all(&repos.linked).unwrap();
    std::fs::rename(&moved, &repos.linked).unwrap();
    git(
        &repos.main,
        &[
            "worktree",
            "move",
            repos.linked.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    assert!(service.get(linked_id).await.is_err());
    let new = service.refresh(project).await.unwrap();
    let moved = new
        .worktrees
        .iter()
        .find(|tree| {
            tree.path == agent_workspace_backend::environment::canonical_path(&moved).unwrap()
        })
        .unwrap();
    assert!(moved.is_available);
    assert_ne!(moved.id, linked_id);
}

#[tokio::test]
async fn fresh_service_has_stable_ids_and_periodic_refresh_isolates_bad_repo() {
    let repos = Repositories::new();
    let (registry, project) = registered(&repos).await;
    let first = WorktreeService::new(registry.clone());
    let before = first.refresh(project).await.unwrap();
    let service = WorktreeService::new(registry.clone());
    let after = service.refresh(project).await.unwrap();
    assert_eq!(
        before
            .worktrees
            .iter()
            .map(|tree| tree.id)
            .collect::<Vec<_>>(),
        after
            .worktrees
            .iter()
            .map(|tree| tree.id)
            .collect::<Vec<_>>()
    );
    let bad = repos.directory.path().join("bad");
    std::fs::create_dir(&bad).unwrap();
    git(&bad, &["init"]);
    let bad = registry
        .add(NewProject {
            root_path: bad.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    std::fs::rename(&bad.root_path, repos.directory.path().join("bad-moved")).unwrap();
    let background = service.clone();
    let task = tokio::spawn(async move {
        background
            .run_refresh_loop(std::time::Duration::from_millis(50))
            .await
    });
    let external = repos.directory.path().join("periodic");
    git(
        &repos.main,
        &[
            "worktree",
            "add",
            "-b",
            "periodic",
            external.to_str().unwrap(),
        ],
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(if cfg!(windows) { 20 } else { 3 }),
        async {
            loop {
                let snapshot = service.list(project).await.unwrap();
                if snapshot.worktrees.len() == 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(
        service.list(bad.id).await.unwrap().error.unwrap().code,
        "worktree_unavailable"
    );
    task.abort();
    let _ = task.await;
    registry.remove(project).await.unwrap();
    assert!(service.list(project).await.is_err());
}

#[tokio::test]
async fn bare_and_locked_worktrees_are_distinguished() {
    let repos = Repositories::new();
    let (registry, project) = registered(&repos).await;
    git(
        &repos.main,
        &[
            "worktree",
            "lock",
            "--reason",
            "do not remove",
            repos.linked.to_str().unwrap(),
        ],
    );
    let bare = repos.directory.path().join("bare.git");
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare"]);
    let bare = registry
        .add(NewProject {
            root_path: bare.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let service = WorktreeService::new(registry);
    let snapshot = service.refresh(project).await.unwrap();
    let linked = snapshot
        .worktrees
        .iter()
        .find(|tree| tree.path == repos.linked)
        .unwrap();
    assert!(linked.is_locked && linked.is_available);
    let snapshot = service.refresh(bare.id).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert!(
        snapshot.worktrees[0].is_main
            && snapshot.worktrees[0].is_bare
            && snapshot.worktrees[0].is_available
    );
}
