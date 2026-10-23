mod common;
use agent_workspace_backend::{
    edit::{execute, Request},
    projects::{NewProject, Registry},
    service::WorkspaceService,
};
use common::Repositories;

async fn scope(repos: &Repositories, path: &str) -> (WorkspaceService, Request) {
    let service =
        WorkspaceService::new(Registry::open(repos.directory.path().join("d9.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let tree = service
        .worktrees
        .refresh(project.project.id)
        .await
        .unwrap()
        .worktrees
        .into_iter()
        .find(|t| t.is_main)
        .unwrap();
    (
        service,
        Request {
            project_id: project.project.id,
            worktree_id: tree.id,
            path: path.into(),
            version: None,
            text: None,
        },
    )
}
fn link(target: &std::path::Path, alias: &std::path::Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, alias).unwrap();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(alias)
        .arg(target)
        .output()
        .unwrap()
        .status
        .success());
}
fn unlink(alias: &std::path::Path) {
    #[cfg(unix)]
    std::fs::remove_file(alias).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(alias).unwrap();
}
#[tokio::test]
async fn links_inside_and_outside_never_authorize_edit() {
    let repos = Repositories::new();
    assert!(repos.linked.is_dir());
    let index = std::fs::read(repos.main.join(".git/index")).ok();
    let inside = repos.main.join("real Юникод & [x]");
    let outside = repos.directory.path().join("outside");
    for path in [&inside, &outside] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("sentinel.txt"), "untouched").unwrap();
    }
    let (service, mut request) = scope(&repos, "alias/sentinel.txt").await;
    let alias = repos.main.join("alias");
    for target in [&inside, &outside] {
        link(target, &alias);
        assert!(execute(&service, request.clone()).await.is_err());
        request.text = Some("forbidden".into());
        assert!(execute(&service, request.clone()).await.is_err());
        request.text = None;
        assert_eq!(
            std::fs::read(target.join("sentinel.txt")).unwrap(),
            b"untouched"
        );
        unlink(&alias);
    }
    assert_eq!(std::fs::read(repos.main.join(".git/index")).ok(), index);
}
#[tokio::test]
async fn same_bytes_replacement_missing_path_and_root_identity() {
    let repos = Repositories::new();
    let file = repos.main.join("Юникод & [x] #.txt");
    std::fs::write(&file, "before").unwrap();
    let (service, mut request) = scope(&repos, "Юникод & [x] #.txt").await;
    let loaded = execute(&service, request.clone()).await.unwrap();
    request.version = Some(loaded.version);
    request.text = Some("forbidden".into());
    std::fs::rename(&file, repos.main.join("old-file")).unwrap();
    assert!(execute(&service, request.clone()).await.is_err());
    std::fs::write(&file, "before").unwrap();
    assert_eq!(
        execute(&service, request.clone()).await.unwrap_err().0,
        "edit_conflict"
    );
    request.text = None;
    request.version = None;
    let loaded = execute(&service, request.clone()).await.unwrap();
    request.version = Some(loaded.version);
    request.text = Some("forbidden".into());
    // Keep the exact Git admin and file objects but replace their root directory.
    let moved = repos.directory.path().join("moved-main");
    std::fs::rename(&repos.main, &moved).unwrap();
    std::fs::create_dir(&repos.main).unwrap();
    std::fs::rename(moved.join(".git"), repos.main.join(".git")).unwrap();
    std::fs::rename(moved.join("Юникод & [x] #.txt"), &file).unwrap();
    let snapshot = service.worktrees.refresh(request.project_id).await.unwrap();
    assert!(snapshot
        .worktrees
        .iter()
        .all(|t| t.id != request.worktree_id || !t.is_available));
    assert!(execute(&service, request).await.is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"before");
}
#[tokio::test]
async fn real_read_and_write_permission_denial() {
    let repos = Repositories::new();
    let path = repos.main.join("denied.txt");
    std::fs::write(&path, "safe").unwrap();
    let (service, request) = scope(&repos, "denied.txt").await;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            unsafe { libc::geteuid() },
            0,
            "Run permission acceptance as unprivileged user"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
        let result = execute(&service, request.clone()).await;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(result.is_err());
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .unwrap();
        let value = String::from_utf8(output.stdout).unwrap();
        let sid = value
            .trim()
            .split(',')
            .next_back()
            .unwrap()
            .trim_matches('"');
        assert!(sid.starts_with("S-1-"));
        let denied = std::process::Command::new("icacls.exe")
            .arg(&path)
            .arg("/deny")
            .arg(format!("*{sid}:(R,W)"))
            .output()
            .unwrap();
        assert!(denied.status.success());
        let result = execute(&service, request.clone()).await;
        let restored = std::process::Command::new("icacls.exe")
            .arg(&path)
            .args(["/remove:d", &format!("*{sid}")])
            .output()
            .unwrap();
        assert!(restored.status.success());
        assert!(result.is_err());
    }
    let loaded = execute(&service, request.clone()).await.unwrap();
    let original = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    let result = execute(
        &service,
        Request {
            version: Some(loaded.version),
            text: Some("forbidden".into()),
            ..request
        },
    )
    .await;
    std::fs::set_permissions(&path, original).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"safe");
}
