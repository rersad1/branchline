mod common;

use agent_workspace_backend::{
    config::Project,
    projects::{validate_registration, ProjectError},
};
use common::{git, Repositories};

#[tokio::test]
async fn canonical_common_dir_rejects_linked_and_symlink_duplicates() {
    let repos = Repositories::new();
    let nested = repos.main.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let resolved = validate_registration(nested.to_str().unwrap(), &[])
        .await
        .unwrap();
    assert_eq!(resolved.root_path, repos.main);
    assert_eq!(resolved.git_common_dir, repos.main.join(".git"));
    let linked = validate_registration(repos.linked.to_str().unwrap(), &[])
        .await
        .unwrap();
    assert_eq!(linked.root_path, repos.linked);
    assert_eq!(linked.git_common_dir, resolved.git_common_dir);
    let existing = vec![Project {
        id: uuid::Uuid::new_v4(),
        name: "fixture".into(),
        root_path: resolved.root_path,
        git_common_dir: resolved.git_common_dir,
        base_ref: None,
        created_at: chrono::Utc::now(),
    }];
    assert!(matches!(
        validate_registration(repos.linked.to_str().unwrap(), &existing).await,
        Err(ProjectError::Duplicate)
    ));
    #[cfg(unix)]
    {
        let alias = repos.directory.path().join("alias");
        std::os::unix::fs::symlink(&repos.main, &alias).unwrap();
        assert!(matches!(
            validate_registration(alias.to_str().unwrap(), &existing).await,
            Err(ProjectError::Duplicate)
        ));
    }
}

#[tokio::test]
async fn invalid_missing_file_and_non_git_paths_fail() {
    for path in ["relative", "C:repo", "//server/repo", "/tmp/invalid\0path"] {
        assert!(matches!(
            validate_registration(path, &[]).await,
            Err(ProjectError::InvalidPath)
        ));
    }
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        validate_registration(directory.path().join("missing").to_str().unwrap(), &[]).await,
        Err(ProjectError::Unavailable)
    ));
    let file = directory.path().join("file");
    std::fs::write(&file, "fixture").unwrap();
    assert!(matches!(
        validate_registration(file.to_str().unwrap(), &[]).await,
        Err(ProjectError::Unavailable)
    ));
    assert!(matches!(
        validate_registration(directory.path().to_str().unwrap(), &[]).await,
        Err(ProjectError::NotRepository)
    ));
}

#[tokio::test]
async fn bare_repository_is_supported() {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "--bare"]);
    let resolved = validate_registration(directory.path().to_str().unwrap(), &[])
        .await
        .unwrap();
    assert_eq!(
        resolved.root_path,
        agent_workspace_backend::environment::canonical_path(directory.path()).unwrap()
    );
    assert_eq!(
        resolved.git_common_dir,
        agent_workspace_backend::environment::canonical_path(directory.path()).unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn permission_denied_directory_is_unavailable() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let denied = directory.path().join("denied");
    std::fs::create_dir(&denied).unwrap();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = validate_registration(denied.to_str().unwrap(), &[]).await;
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
    if std::env::var("USER").as_deref() == Ok("root") {
        eprintln!("permission check requires an unprivileged Linux user");
        return;
    }
    assert!(matches!(result, Err(ProjectError::Unavailable)));
}
