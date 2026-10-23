mod common;
use agent_workspace_backend::{
    edit::{execute, Request},
    projects::{NewProject, Registry},
    service::WorkspaceService,
};
use common::{git, Repositories};

async fn scope(repos: &Repositories, path: &str) -> (WorkspaceService, Request) {
    let service = WorkspaceService::new(
        Registry::open(repos.directory.path().join("edit-config.json")).unwrap(),
    );
    let p = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let t = service
        .worktrees
        .refresh(p.project.id)
        .await
        .unwrap()
        .worktrees
        .into_iter()
        .find(|t| t.is_main)
        .unwrap();
    (
        service,
        Request {
            project_id: p.project.id,
            worktree_id: t.id,
            path: path.into(),
            version: None,
            text: None,
        },
    )
}
#[tokio::test]
async fn full_file_bom_crlf_unicode_untracked_and_unchanged_index() {
    let repos = Repositories::new();
    std::fs::create_dir(repos.main.join("каталог с пробелом")).unwrap();
    let path = "каталог с пробелом/новый файл.txt";
    let text = (0..300)
        .map(|i| format!("строка {i}\r\n"))
        .collect::<String>();
    std::fs::write(
        repos.main.join(path),
        [&[239, 187, 191][..], text.as_bytes()].concat(),
    )
    .unwrap();
    let before = std::fs::read(repos.main.join(".git/index")).unwrap();
    let (service, mut r) = scope(&repos, path).await;
    let loaded = execute(&service, r.clone()).await.unwrap();
    assert_eq!(loaded.text.lines().count(), 300);
    assert_eq!(loaded.original, "");
    r.version = Some(loaded.version);
    r.text = Some(loaded.text.replace("строка 150", "изменено 150"));
    let saved = execute(&service, r.clone()).await.unwrap();
    let bytes = std::fs::read(repos.main.join(path)).unwrap();
    assert!(bytes.starts_with(&[239, 187, 191]));
    assert_eq!(
        String::from_utf8(bytes[3..].to_vec())
            .unwrap()
            .matches("\r\n")
            .count(),
        300
    );
    assert!(saved.text.contains("строка 299"));
    assert_eq!(
        std::fs::read(repos.main.join(".git/index")).unwrap(),
        before
    );
    r.text = Some("late".into());
    assert_eq!(execute(&service, r).await.unwrap_err().0, "edit_conflict");
}
#[tokio::test]
async fn tracked_external_conflict_serialization_error_and_path_guards() {
    let repos = Repositories::new();
    std::fs::write(repos.main.join("file.txt"), "index\n").unwrap();
    git(&repos.main, &["add", "file.txt"]);
    std::fs::write(repos.main.join("file.txt"), "working\n").unwrap();
    let (service, mut r) = scope(&repos, "file.txt").await;
    let load = execute(&service, r.clone()).await.unwrap();
    assert_eq!(load.original, "index\n");
    r.version = Some(load.version);
    r.text = Some("mine\n".into());
    std::fs::write(repos.main.join("file.txt"), "external\n").unwrap();
    assert_eq!(
        execute(&service, r.clone()).await.unwrap_err().0,
        "edit_conflict"
    );
    assert_eq!(
        std::fs::read_to_string(repos.main.join("file.txt")).unwrap(),
        "external\n"
    );
    r.version = None;
    r.text = None;
    let load = execute(&service, r.clone()).await.unwrap();
    r.version = Some(load.version);
    r.text = Some("first\n".into());
    let mut other = r.clone();
    other.text = Some("second\n".into());
    let (a, b) = tokio::join!(execute(&service, r.clone()), execute(&service, other));
    assert_ne!(a.is_ok(), b.is_ok());
    r.text = None;
    r.version = None;
    for path in [
        "../escape",
        ".git/index",
        ".git./index",
        "sub/../../escape",
        "file.txt:stream",
    ] {
        let mut invalid = r.clone();
        invalid.path = path.into();
        assert!(execute(&service, invalid).await.is_err(), "{path}");
    }
    for (path, bytes, code) in [
        ("invalid.txt", &b"\xffbad"[..], "edit_encoding"),
        ("binary.txt", &b"a\0b"[..], "edit_encoding"),
    ] {
        std::fs::write(repos.main.join(path), bytes).unwrap();
        let mut invalid = r.clone();
        invalid.path = path.into();
        assert_eq!(execute(&service, invalid).await.unwrap_err().0, code);
    }
    let mut mixed = r.clone();
    mixed.path = "mixed.txt".into();
    std::fs::write(repos.main.join("mixed.txt"), b"a\r\nb\nc\rd").unwrap();
    let loaded = execute(&service, mixed.clone()).await.unwrap();
    mixed.version = Some(loaded.version);
    mixed.text = Some("a\r\nchanged\nc\rd".into());
    execute(&service, mixed).await.unwrap();
    assert_eq!(
        std::fs::read(repos.main.join("mixed.txt")).unwrap(),
        b"a\r\nchanged\nc\rd"
    );
    std::fs::write(repos.main.join("large.txt"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let mut large = r.clone();
    large.path = "large.txt".into();
    assert_eq!(execute(&service, large).await.unwrap_err().0, "edit_large");
    let load = execute(&service, r.clone()).await.unwrap();
    r.version = Some(load.version);
    r.text = Some("read only".into());
    let path = repos.main.join("file.txt");
    let original = std::fs::metadata(&path).unwrap().permissions();
    let mut readonly = original.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&path, readonly).unwrap();
    assert_eq!(execute(&service, r).await.unwrap_err().0, "edit_io");
    std::fs::set_permissions(&path, original).unwrap();
}
#[tokio::test]
async fn rejects_hardlinks_and_substituted_directories() {
    let repos = Repositories::new();
    std::fs::write(repos.main.join("target.txt"), "safe\n").unwrap();
    std::fs::hard_link(repos.main.join("target.txt"), repos.main.join("alias.txt")).unwrap();
    let (service, mut r) = scope(&repos, "alias.txt").await;
    assert_eq!(
        execute(&service, r.clone()).await.unwrap_err().0,
        "edit_path"
    );
    std::fs::create_dir(repos.main.join("dir")).unwrap();
    std::fs::write(repos.main.join("dir/file.txt"), "safe\n").unwrap();
    r.path = "dir/file.txt".into();
    let load = execute(&service, r.clone()).await.unwrap();
    r.text = Some("mine\n".into());
    r.version = Some(load.version);
    std::fs::rename(repos.main.join("dir"), repos.main.join("old-dir")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(repos.main.join("old-dir"), repos.main.join("dir")).unwrap();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(repos.main.join("dir"))
        .arg(repos.main.join("old-dir"))
        .output()
        .unwrap()
        .status
        .success());
    assert!(execute(&service, r).await.is_err());
    assert_eq!(
        std::fs::read_to_string(repos.main.join("old-dir/file.txt")).unwrap(),
        "safe\n"
    );
}
