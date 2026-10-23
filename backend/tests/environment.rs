mod common;
use agent_workspace_backend::{
    environment::{valid_path, ExecutionEnvironment},
    projects::{NewProject, Registry},
    service::WorkspaceService,
};

#[tokio::test]
async fn local_boundary_projects_snapshots_and_registry_v1_keep_identity() {
    let repos = common::Repositories::new();
    let path = repos.directory.path().join("config.json");
    let environment = ExecutionEnvironment::local();
    let mut other = environment.clone();
    other.id = uuid::Uuid::new_v4();
    assert!(Registry::open_in(path.clone(), other).is_err());
    let mut other_distro = environment.clone();
    other_distro.distro = Some("UnselectedDistro".into());
    assert!(Registry::open_in(path.clone(), other_distro).is_err());
    let service =
        WorkspaceService::new(Registry::open_in(path.clone(), environment.clone()).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    assert_eq!(project.environment_id, environment.id);
    let snapshot = service.worktrees.refresh(project.project.id).await.unwrap();
    assert_eq!(snapshot.environment_id, environment.id);
    assert!(snapshot
        .worktrees
        .iter()
        .all(|t| t.environment_id == environment.id && t.is_available));
    assert!(snapshot.worktrees.iter().any(|t| t.path == repos.linked));
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored["version"], 1);
    assert!(stored["projects"][0].get("environment_id").is_none());
    drop(service);
    let original = std::fs::read(&path).unwrap();
    let restored = Registry::open(path.clone()).unwrap();
    assert_eq!(restored.list().await.unwrap()[0].id, project.project.id);
    assert_eq!(
        std::fs::read(path.with_extension("json.v1.bak")).unwrap(),
        original
    );
    assert!(Registry::open(path.clone()).is_err());
    drop(restored);
    let service = WorkspaceService::new(Registry::open(path.clone()).unwrap());
    let app = agent_workspace_backend::router(agent_workspace_backend::app::AppState::new(
        service.registry.clone(),
        agent_workspace_backend::app::LocalAuth::new("a".repeat(32), 43871).unwrap(),
    ));
    use tower::ServiceExt;
    for (id, expected) in [(environment.id, 200), (uuid::Uuid::new_v4(), 400)] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v1/projects?environment_id={id}"))
                    .header("Authorization", format!("Bearer {}", "a".repeat(32)))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
}

#[test]
fn paths_are_validated_for_the_execution_host() {
    for path in [
        "relative",
        "C:relative",
        "//server/share",
        "\\\\wsl.localhost\\Ubuntu\\home\\repo",
        "bad\0path",
    ] {
        assert!(!valid_path(path));
    }
    #[cfg(windows)]
    {
        assert!(valid_path("C:/repo"));
        assert!(valid_path(r"\\?\C:\repo"));
        assert!(!valid_path("/home/repo"));
    }
    #[cfg(unix)]
    {
        assert!(valid_path("/home/repo"));
        assert!(!valid_path("C:/repo"));
    }
}

#[cfg(windows)]
#[tokio::test]
async fn windows_case_sensitive_siblings_are_distinct_repositories() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sensitive");
    std::fs::create_dir(&root).unwrap();
    let output = std::process::Command::new("fsutil.exe")
        .args(["file", "setCaseSensitiveInfo"])
        .arg(&root)
        .arg("enable")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Fixture requires NTFS case-sensitive directory support"
    );
    let upper = root.join("Repo");
    let lower = root.join("repo");
    std::fs::create_dir(&upper).unwrap();
    std::fs::create_dir(&lower).unwrap();
    common::git(&upper, &["init"]);
    common::git(&lower, &["init"]);
    let registry = Registry::open(directory.path().join("config.json")).unwrap();
    let first = registry
        .add(NewProject {
            root_path: upper.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let second = registry
        .add(NewProject {
            root_path: lower.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_ne!(first.git_common_dir, second.git_common_dir);
    let service = WorkspaceService::new(registry);
    let a = service.worktrees.refresh(first.id).await.unwrap();
    let b = service.worktrees.refresh(second.id).await.unwrap();
    assert!(a.worktrees[0].is_available && b.worktrees[0].is_available);
    assert_ne!(a.worktrees[0].id, b.worktrees[0].id);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_case_junction_long_paths_and_admin_recreation() {
    let repos = common::Repositories::new();
    let first =
        agent_workspace_backend::projects::validate_registration(repos.main.to_str().unwrap(), &[])
            .await
            .unwrap();
    let case_alias = repos.main.to_str().unwrap().to_uppercase();
    let second = agent_workspace_backend::projects::validate_registration(&case_alias, &[])
        .await
        .unwrap();
    assert_eq!(first, second);
    let alias = repos.directory.path().join("junction");
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&repos.main)
        .output()
        .unwrap();
    assert!(result.status.success());
    let aliased =
        agent_workspace_backend::projects::validate_registration(alias.to_str().unwrap(), &[])
            .await
            .unwrap();
    assert_eq!(first, aliased);
    // Remove only the junction itself, before TempDir recursive fixture cleanup.
    std::fs::remove_dir(&alias).unwrap();
    let long = repos
        .directory
        .path()
        .join("long-segment-".repeat(12))
        .join("nested-segment-".repeat(10));
    std::fs::create_dir_all(&long).unwrap();
    assert!(matches!(
        agent_workspace_backend::projects::validate_registration(long.to_str().unwrap(), &[]).await,
        Err(agent_workspace_backend::projects::ProjectError::UnsupportedPath)
    ));
    let service =
        WorkspaceService::new(Registry::open(repos.directory.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let before = service.worktrees.refresh(project.project.id).await.unwrap();
    let old = before
        .worktrees
        .iter()
        .find(|t| t.path == repos.linked)
        .unwrap()
        .id;
    common::git(
        &repos.main,
        &["worktree", "remove", repos.linked.to_str().unwrap()],
    );
    common::git(
        &repos.main,
        &["worktree", "add", repos.linked.to_str().unwrap(), "linked"],
    );
    let after = service.worktrees.refresh(project.project.id).await.unwrap();
    assert_ne!(
        old,
        after
            .worktrees
            .iter()
            .find(|t| t.path == repos.linked)
            .unwrap()
            .id
    );
    assert!(service.worktrees.get(old).await.is_err());
}
