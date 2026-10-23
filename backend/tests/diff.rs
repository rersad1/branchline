mod common;
use agent_workspace_backend::{
    diff::{DiffError, DiffRequest, Mode},
    projects::{NewProject, Registry},
    service::WorkspaceService,
};
use common::{git, Repositories};
use uuid::Uuid;
fn commit(path: &std::path::Path) {
    git(
        path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    );
}
async fn scope(repos: &Repositories) -> (WorkspaceService, DiffRequest) {
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
        DiffRequest {
            project_id: project.project.id,
            worktree_id: tree.id,
            mode: Mode::Unstaged,
            base_ref: None,
            file_id: None,
            context_lines: 3,
        },
    )
}
#[tokio::test]
async fn untracked_list_counts_without_opening_patches_and_refreshes_without_index_writes() {
    let repos = Repositories::new();
    for i in 0..200 {
        std::fs::write(
            repos.main.join(format!("new-{i:03}.txt")),
            "one\r\ntwo\nthree",
        )
        .unwrap();
    }
    std::fs::write(repos.main.join("empty"), "").unwrap();
    std::fs::write(repos.main.join("binary"), b"one\0two\n").unwrap();
    std::fs::write(repos.main.join("invalid-utf8"), b"\xff\n").unwrap();
    std::fs::write(repos.main.join("large"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let before = std::fs::read(repos.main.join(".git/index")).ok();
    let (service, request) = scope(&repos).await;
    let list = service.diff(request.clone()).await.unwrap();
    assert!(list.selected_file.is_none());
    assert_eq!(list.files.len(), 204);
    for file in &list.files {
        assert_eq!(file.status, "?");
        assert_eq!(file.deletions, Some(0));
        match file.path.as_str() {
            "binary" | "invalid-utf8" => {
                assert_eq!(file.binary, Some(true));
                assert_eq!(file.additions, None);
            }
            "large" => assert_eq!(file.additions, None),
            "empty" => assert_eq!(file.additions, Some(0)),
            _ => {
                assert_eq!(file.additions, Some(3), "{}", file.path);
                assert_eq!(file.binary, Some(false));
            }
        }
    }
    std::fs::write(repos.main.join("new-000.txt"), "changed\n").unwrap();
    let refreshed = service.diff(request.clone()).await.unwrap();
    assert_eq!(
        refreshed
            .files
            .iter()
            .find(|f| f.path == "new-000.txt")
            .unwrap()
            .additions,
        Some(1)
    );
    let mut detail_request = request;
    detail_request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "new-001.txt")
            .unwrap()
            .id,
    );
    let detail = service.diff(detail_request).await.unwrap();
    assert_eq!(detail.selected_file.unwrap().state, "text");
    assert_eq!(std::fs::read(repos.main.join(".git/index")).ok(), before);
}

#[tokio::test]
async fn unmerged_duplicate_name_status_has_one_conflict_identity_and_unknown_counts() {
    let repos = Repositories::new();
    let file = repos.main.join("conflict.txt");
    std::fs::write(&file, "base\n").unwrap();
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    git(&repos.main, &["checkout", "-b", "theirs"]);
    std::fs::write(&file, "theirs\n").unwrap();
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    git(&repos.main, &["checkout", "main"]);
    std::fs::write(&file, "ours\n").unwrap();
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(&repos.main)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "merge",
            "--no-edit",
            "theirs",
        ])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1));
    let before = std::fs::read(repos.main.join(".git/index")).unwrap();
    let (service, mut request) = scope(&repos).await;
    for deleted in [false, true] {
        if deleted {
            std::fs::remove_file(&file).unwrap();
        }
        let raw = std::process::Command::new("git")
            .arg("-C")
            .arg(&repos.main)
            .args(["diff", "--name-status", "-z"])
            .output()
            .unwrap();
        assert!(raw.status.success());
        assert_eq!(
            raw.stdout
                .split(|b| *b == 0)
                .filter(|field| *field == b"conflict.txt")
                .count(),
            2
        );
        request.file_id = None;
        let list = service.diff(request.clone()).await.unwrap();
        assert_eq!(list.files.len(), 1);
        assert_eq!(list.files[0].status, "U");
        assert_eq!(list.files[0].additions, None);
        assert_eq!(list.files[0].deletions, None);
        request.file_id = Some(list.files[0].id);
        let detail = service
            .diff(request.clone())
            .await
            .unwrap()
            .selected_file
            .unwrap();
        assert_eq!(detail.state, "conflict");
        assert!(detail.patch.is_none());
        assert_eq!(
            std::fs::read(repos.main.join(".git/index")).unwrap(),
            before
        );
    }
}
#[tokio::test]
async fn modes_rename_binary_untracked_literal_names_and_index_preservation() {
    let repos = Repositories::new();
    for (name, bytes) in [
        ("tracked.txt", b"base\n".as_slice()),
        ("old.txt", b"rename\n"),
        ("binary.dat", b"\0one"),
        ("delete.txt", b"delete\n"),
        ("wild[1].txt", b"base\n"),
    ] {
        std::fs::write(repos.main.join(name), bytes).unwrap();
    }
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    git(&repos.main, &["branch", "base"]);
    std::fs::write(repos.main.join("tracked.txt"), "index\n").unwrap();
    git(&repos.main, &["add", "tracked.txt"]);
    git(&repos.main, &["mv", "old.txt", "new name.txt"]);
    git(&repos.main, &["rm", "delete.txt"]);
    std::fs::write(repos.main.join("tracked.txt"), "worktree\n").unwrap();
    std::fs::write(repos.main.join("binary.dat"), b"\0two").unwrap();
    std::fs::write(repos.main.join("wild[1].txt"), "literal path\n").unwrap();
    std::fs::write(repos.main.join("untracked.txt"), "new content").unwrap();
    let before = std::fs::read(repos.main.join(".git/index")).unwrap();
    let (service, mut request) = scope(&repos).await;
    assert!(service
        .worktrees
        .refresh(request.project_id)
        .await
        .unwrap()
        .worktrees
        .iter()
        .any(|t| t.path == repos.linked));
    let list = service.diff(request.clone()).await.unwrap();
    assert!(list.selected_file.is_none());
    assert!(list
        .files
        .iter()
        .any(|f| f.path == "untracked.txt" && f.status == "?"));
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "tracked.txt")
            .unwrap()
            .id,
    );
    let patch = service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap()
        .patch
        .unwrap();
    assert!(patch.contains("-index\n+worktree"));
    assert!(!patch.contains("-base"));
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "wild[1].txt")
            .unwrap()
            .id,
    );
    assert!(service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap()
        .patch
        .unwrap()
        .contains("+literal path"));
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "binary.dat")
            .unwrap()
            .id,
    );
    let binary = service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap();
    assert_eq!(binary.state, "binary");
    assert!(binary.patch.is_none());
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "untracked.txt")
            .unwrap()
            .id,
    );
    let untracked = service.diff(request.clone()).await.unwrap();
    assert_eq!(
        untracked
            .files
            .iter()
            .find(|f| f.path == "untracked.txt")
            .unwrap()
            .additions,
        Some(1)
    );
    assert!(untracked
        .selected_file
        .unwrap()
        .patch
        .unwrap()
        .contains("+new content\n\\ No newline"));
    request.file_id = None;
    request.mode = Mode::Staged;
    let staged = service.diff(request.clone()).await.unwrap();
    let rename = staged
        .files
        .iter()
        .find(|f| f.path == "new name.txt")
        .unwrap();
    assert_eq!(rename.original_path.as_deref(), Some("old.txt"));
    assert_eq!(rename.status, "R");
    assert!(staged
        .files
        .iter()
        .any(|f| f.status == "D" && f.path == "delete.txt"));
    request.file_id = Some(
        staged
            .files
            .iter()
            .find(|f| f.path == "tracked.txt")
            .unwrap()
            .id,
    );
    assert!(service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap()
        .patch
        .unwrap()
        .contains("-base\n+index"));
    assert_eq!(
        std::fs::read(repos.main.join(".git/index")).unwrap(),
        before
    );
    request.file_id = Some(list.files[0].id);
    assert!(matches!(
        service.diff(request.clone()).await,
        Err(DiffError::FileNotFound)
    ));
    request.file_id = None;
    request.context_lines = 101;
    assert!(matches!(
        service.diff(request).await,
        Err(DiffError::Invalid)
    ));
}
#[tokio::test]
async fn branch_uses_merge_base_and_excludes_local_changes_no_base_is_explicit() {
    let repos = Repositories::new();
    std::fs::write(repos.main.join("tracked"), "base\n").unwrap();
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    git(&repos.main, &["branch", "base"]);
    std::fs::write(repos.main.join("tracked"), "committed\n").unwrap();
    git(&repos.main, &["add", "."]);
    commit(&repos.main);
    std::fs::write(repos.main.join("tracked"), "local staged\n").unwrap();
    git(&repos.main, &["add", "."]);
    std::fs::write(repos.main.join("tracked"), "local unstaged\n").unwrap();
    let before = std::fs::read(repos.main.join(".git/index")).unwrap();
    let (service, mut request) = scope(&repos).await;
    request.mode = Mode::Branch;
    request.base_ref = Some("base".into());
    let list = service.diff(request.clone()).await.unwrap();
    assert!(list.base_commit.is_some());
    request.file_id = Some(list.files[0].id);
    let patch = service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap()
        .patch
        .unwrap();
    assert!(patch.contains("-base\n+committed"));
    assert!(!patch.contains("local staged"));
    assert!(!patch.contains("local unstaged"));
    request.file_id = None;
    request.base_ref = Some("missing-reference".into());
    assert_eq!(
        service.diff(request.clone()).await.unwrap().state,
        "no_base"
    );
    request.base_ref = Some("--output=escape".into());
    assert!(matches!(
        service.diff(request).await,
        Err(DiffError::Invalid)
    ));
    assert_eq!(
        std::fs::read(repos.main.join(".git/index")).unwrap(),
        before
    );
}
#[tokio::test]
async fn unborn_bare_missing_and_foreign_file_ids_are_not_successful_clean() {
    let repos = Repositories::new();
    let (service, mut request) = scope(&repos).await;
    let unborn = repos.directory.path().join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init"]);
    std::fs::write(unborn.join("initial"), "new\n").unwrap();
    git(&unborn, &["add", "."]);
    let project = service
        .add(NewProject {
            root_path: unborn.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    request.project_id = project.project.id;
    request.worktree_id = service
        .worktrees
        .refresh(request.project_id)
        .await
        .unwrap()
        .worktrees[0]
        .id;
    request.mode = Mode::Staged;
    let result = service.diff(request.clone()).await.unwrap();
    assert_eq!(result.state, "unborn");
    assert_eq!(result.files[0].status, "A");
    request.mode = Mode::Branch;
    request.base_ref = Some("main".into());
    assert_eq!(service.diff(request.clone()).await.unwrap().state, "unborn");
    request.mode = Mode::Unstaged;
    request.base_ref = None;
    request.file_id = Some(Uuid::new_v4());
    assert!(matches!(
        service.diff(request.clone()).await,
        Err(DiffError::FileNotFound)
    ));
    request.file_id = None;
    std::fs::rename(&unborn, repos.directory.path().join("moved")).unwrap();
    assert!(matches!(
        service.diff(request.clone()).await,
        Err(DiffError::Unavailable)
    ));
    let bare = repos.directory.path().join("bare.git");
    git(
        repos.directory.path(),
        &["init", "--bare", bare.to_str().unwrap()],
    );
    let project = service
        .add(NewProject {
            root_path: bare.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    request.project_id = project.project.id;
    request.worktree_id = service
        .worktrees
        .refresh(request.project_id)
        .await
        .unwrap()
        .worktrees[0]
        .id;
    assert_eq!(service.diff(request).await.unwrap().state, "bare");
}
#[tokio::test]
async fn untracked_large_file_and_symlink_escape_are_bounded() {
    let repos = Repositories::new();
    std::fs::write(repos.main.join("large"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    // Content fits the limit, but '+' prefixes make the generated patch exceed it.
    std::fs::write(repos.main.join("patch-large"), vec![b'\n'; 600 * 1024]).unwrap();
    let outside = repos.directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret"), "must not read").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, repos.main.join("escape")).unwrap();
    #[cfg(windows)]
    {
        let result = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(repos.main.join("escape"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(result.status.success());
    }
    let (service, mut request) = scope(&repos).await;
    let list = service.diff(request.clone()).await.unwrap();
    request.file_id = Some(list.files.iter().find(|f| f.path == "large").unwrap().id);
    assert_eq!(
        service
            .diff(request.clone())
            .await
            .unwrap()
            .selected_file
            .unwrap()
            .state,
        "large_file"
    );
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path == "patch-large")
            .unwrap()
            .id,
    );
    let detail = service
        .diff(request.clone())
        .await
        .unwrap()
        .selected_file
        .unwrap();
    assert_eq!(detail.state, "large_file");
    assert!(detail.patch.is_none());
    request.file_id = Some(
        list.files
            .iter()
            .find(|f| f.path.starts_with("escape"))
            .unwrap()
            .id,
    );
    assert!(matches!(
        service.diff(request).await,
        Err(DiffError::Unavailable)
    ));
}
