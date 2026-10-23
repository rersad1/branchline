//! Process-boundary regression only. Real-editor acceptance is a separate app test.
use agent_workspace_backend::{
    create_worktree,
    projects::{NewProject, Registry},
    service::WorkspaceService,
    terminal::{self, Action, Request},
};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use uuid::Uuid;
fn git(path: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn main() {
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("--new-window" | "--remote")
    ) {
        let cwd = std::env::current_dir().unwrap();
        let report = serde_json::json!({"cwd":cwd,"argv":std::env::args().skip(1).collect::<Vec<_>>(),"pid":std::process::id(),"private_token":std::env::var_os("AWM_TOKEN").is_some()});
        std::fs::write(
            cwd.join("ide-report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) && !cwd.join("ide-exit").exists() {
            std::fs::write(
                cwd.join("ide-heartbeat"),
                start.elapsed().as_nanos().to_string(),
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        return;
    }
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| tokio::runtime::Runtime::new().unwrap().block_on(check()))
        .unwrap()
        .join()
        .unwrap();
}
async fn check() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo тест");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    let service = WorkspaceService::new(Registry::open(root.path().join("app.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repo.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap()
        .project;
    let path = root.path().join("linked Ю & $ space");
    let created = create_worktree::create(
        &service,
        create_worktree::CreateRequest {
            project_id: project.id,
            base_ref: "main".into(),
            branch: "linked".into(),
            path: path.to_str().unwrap().into(),
        },
    )
    .await
    .unwrap();
    let tree = created.worktree_id.unwrap();
    let id = Uuid::new_v4();
    let exe = root.path().join(if cfg!(windows) {
        "IDE & $ Ю.exe"
    } else {
        "IDE & $ Ю"
    });
    std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
    let request = |executable, request_id| Request {
        project_id: project.id,
        worktree_id: tree,
        action: Action::OpenIde {
            request_id,
            executable,
        },
    };
    assert_eq!(
        terminal::execute(
            &service,
            request(root.path().join("missing"), Uuid::new_v4())
        )
        .await
        .unwrap_err()
        .0,
        "terminal_ide_executable"
    );
    assert_eq!(
        terminal::execute(&service, request(exe.clone(), Uuid::nil()))
            .await
            .unwrap_err()
            .0,
        "terminal_input"
    );
    let unsupported = root.path().join("unsupported.cmd");
    std::fs::write(&unsupported, "no shell interpolation").unwrap();
    assert_eq!(
        terminal::execute(&service, request(unsupported, Uuid::new_v4()))
            .await
            .unwrap_err()
            .0,
        "terminal_ide_executable"
    );
    let mut foreign = request(exe.clone(), Uuid::new_v4());
    foreign.project_id = Uuid::new_v4();
    assert!(terminal::execute(&service, foreign).await.is_err());
    let response = terminal::execute(&service, request(exe.clone(), id))
        .await
        .unwrap();
    assert!(response.session_id.is_none());
    assert!(response.sessions.is_empty());
    let start = Instant::now();
    while !path.join("ide-report.json").exists() {
        assert!(start.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let first = std::fs::read(path.join("ide-report.json")).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&first).unwrap();
    #[cfg(windows)]
    let expected = serde_json::json!(["--new-window", path]);
    #[cfg(unix)]
    let expected = serde_json::json!([
        "--remote",
        format!("wsl+{}", service.environment.distro.as_deref().unwrap()),
        "--new-window",
        path
    ]);
    assert_eq!(report["argv"], expected);
    assert_eq!(report["cwd"], serde_json::json!(path));
    assert_eq!(report["private_token"], false);
    terminal::execute(&service, request(exe.clone(), id))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(std::fs::read(path.join("ide-report.json")).unwrap(), first);
    service.terminals.close_all();
    let before = std::fs::read(path.join("ide-heartbeat")).unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_ne!(
        std::fs::read(path.join("ide-heartbeat")).unwrap(),
        before,
        "app close terminated external editor"
    );
    assert_eq!(
        terminal::execute(&service, request(exe, Uuid::new_v4()))
            .await
            .unwrap_err()
            .0,
        "terminal_scope"
    );
    std::fs::write(path.join("ide-exit"), "").unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    println!("PASS external IDE: exact argv/cwd/Unicode/metacharacters; missing/unsupported/foreign/nil rejection; request dedup; no PTY/adoption; survives close; no launch after close");
}
