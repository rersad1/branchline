#![cfg_attr(windows, windows_subsystem = "windows")]
use agent_workspace_backend::{
    create_worktree,
    projects::{NewProject, Registry},
    remove_worktree,
    service::WorkspaceService,
    terminal::{self, Action, Request},
};
use std::{path::Path, time::Duration};
use uuid::Uuid;
fn git(path: &Path, args: &[&str]) {
    let output = std::process::Command::new(if cfg!(windows) {
        "git.exe"
    } else {
        "/usr/bin/git"
    })
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
    if std::env::args().nth(1).as_deref() == Some("--terminal-shell") {
        #[cfg(unix)]
        if Path::new(".slow-close-test").exists() {
            slow_supervisor();
            return;
        }
        std::process::exit(agent_workspace_backend::terminal_shell::run());
    }
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_keep_alive(Duration::from_millis(100))
                .build()
                .unwrap()
                .block_on(check())
        })
        .unwrap()
        .join()
        .unwrap();
}
async fn check() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("source");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("tracked"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "base"]);
    let service = WorkspaceService::new(Registry::open(temp.path().join("config.json")).unwrap());
    #[cfg(windows)]
    let service = {
        let mut service = service;
        service.terminals = terminal::TerminalService::with_executable(std::path::PathBuf::from(
            env!("CARGO_BIN_EXE_terminal-host"),
        ));
        service
    };
    let project = service
        .add(NewProject {
            root_path: repo.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap()
        .project;
    let created = create_worktree::create(
        &service,
        create_worktree::CreateRequest {
            project_id: project.id,
            base_ref: "main".into(),
            branch: "fixture/terminal".into(),
            path: temp.path().join("linked Ю").to_str().unwrap().into(),
        },
    )
    .await
    .unwrap();
    assert!(created.git_created);
    let tree = created.worktree_id.unwrap();
    let index = std::fs::read(repo.join(".git/index")).unwrap();
    let request = |action| Request {
        project_id: project.id,
        worktree_id: tree,
        action,
    };
    let id = Uuid::new_v4();
    let open = terminal::execute(
        &service,
        request(Action::Open {
            request_id: id,
            rows: 24,
            cols: 80,
        }),
    )
    .await
    .unwrap();
    assert_eq!(open.sessions.len(), 1);
    assert_eq!(
        terminal::execute(
            &service,
            request(Action::Open {
                request_id: id,
                rows: 24,
                cols: 80
            })
        )
        .await
        .unwrap()
        .sessions
        .len(),
        1
    );
    assert!(matches!(
        service.remove(project.id).await,
        Err(agent_workspace_backend::projects::RegistryError::ActiveTerminal)
    ));
    // Retire the creating blocking thread; the PTY must follow the process lifetime.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let invalid = Request {
        project_id: Uuid::new_v4(),
        worktree_id: tree,
        action: Action::Input {
            session_id: id,
            data: "echo BAD\r".into(),
        },
    };
    assert_eq!(
        terminal::execute(&service, invalid).await.unwrap_err().0,
        "terminal_scope"
    );
    if cfg!(windows) {
        terminal::execute(
            &service,
            request(Action::Input {
                session_id: id,
                data: "\x1b[1;1R".into(),
            }),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let command = if cfg!(windows) {
        "Write-Output ('PTY_' + 'READY_Ю'); (Get-Location).Path\r"
    } else {
        "printf 'PTY_%s\\n' 'READY_Ю'; pwd\n"
    };
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: command.into(),
        }),
    )
    .await
    .unwrap();
    let mut output = vec![];
    let mut cursor = 0;
    for _ in 0..200 {
        let r = terminal::execute(
            &service,
            request(Action::Read {
                session_id: id,
                cursor,
            }),
        )
        .await
        .unwrap();
        cursor = r.cursor;
        output.extend(r.data);
        if String::from_utf8_lossy(&output).contains("PTY_READY_Ю") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        String::from_utf8_lossy(&output).contains("PTY_READY_Ю"),
        "{}",
        String::from_utf8_lossy(&output)
    );
    terminal::execute(
        &service,
        request(Action::Resize {
            session_id: id,
            rows: 31,
            cols: 103,
        }),
    )
    .await
    .unwrap();
    let list = terminal::execute(&service, request(Action::List))
        .await
        .unwrap();
    assert_eq!(list.sessions[0].cols, 103);
    assert_eq!(list.sessions[0].cwd, created.path.to_str().unwrap());
    let remove = || remove_worktree::Request {
        missing_only: false,
        project_id: project.id,
        worktree_id: tree,
        confirm_path: created.path.to_str().unwrap().into(),
        external_sessions_checked: true,
    };
    assert_eq!(
        remove_worktree::remove(&service, remove())
            .await
            .unwrap_err()
            .0,
        "remove_session"
    );
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: if cfg!(windows) {
                "Start-Sleep -Seconds 30\r"
            } else {
                "sleep 30\n"
            }
            .into(),
        }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: "\x03".into(),
        }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: if cfg!(windows) {
                "Write-Output ('CTRL_' + 'SURVIVED')\r"
            } else {
                "printf 'CTRL_%s\n' 'SURVIVED'\n"
            }
            .into(),
        }),
    )
    .await
    .unwrap();
    let mut survived = false;
    let mut control_output = Vec::new();
    for _ in 0..200 {
        let r = terminal::execute(
            &service,
            request(Action::Read {
                session_id: id,
                cursor,
            }),
        )
        .await
        .unwrap();
        cursor = r.cursor;
        control_output.extend(r.data);
        if String::from_utf8_lossy(&control_output).contains("CTRL_SURVIVED") {
            survived = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        survived,
        "Ctrl+C must cancel foreground command without killing supervisor/shell: {}",
        String::from_utf8_lossy(&control_output)
    );
    let flood = if cfg!(windows) {
        "Write-Output ('x' * 350000)\r"
    } else {
        "python3 -c \"print('x'*350000)\"\n"
    };
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: flood.into(),
        }),
    )
    .await
    .unwrap();
    let mut bounded = false;
    for _ in 0..200 {
        let r = terminal::execute(
            &service,
            request(Action::Read {
                session_id: id,
                cursor: 0,
            }),
        )
        .await
        .unwrap();
        assert!(r.data.len() <= 32768);
        if r.lost {
            bounded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        bounded,
        "output ring must remain bounded and report lost bytes"
    );
    terminal::execute(
        &service,
        request(Action::Input {
            session_id: id,
            data: if cfg!(windows) {
                "exit 7\r"
            } else {
                "exit 7\n"
            }
            .into(),
        }),
    )
    .await
    .unwrap();
    let mut exited = false;
    for _ in 0..200 {
        let r = terminal::execute(&service, request(Action::List))
            .await
            .unwrap();
        if r.sessions[0].state == "exited" {
            assert_eq!(r.sessions[0].exit_code, Some(7));
            exited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(exited, "normal shell exit must retain its real exit code");
    terminal::execute(&service, request(Action::Close { session_id: id }))
        .await
        .unwrap();
    assert!(terminal::execute(&service, request(Action::List))
        .await
        .unwrap()
        .sessions
        .is_empty());
    std::fs::write(created.path.join("untracked"), "keep").unwrap();
    assert_eq!(
        remove_worktree::remove(&service, remove())
            .await
            .unwrap_err()
            .0,
        "remove_dirty"
    );
    std::fs::remove_file(created.path.join("untracked")).unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "ignored-file\n").unwrap();
    std::fs::write(created.path.join("ignored-file"), "preserve ignored").unwrap();
    assert_eq!(
        remove_worktree::remove(&service, remove())
            .await
            .unwrap_err()
            .0,
        "remove_dirty"
    );
    assert_eq!(
        std::fs::read_to_string(created.path.join("ignored-file")).unwrap(),
        "preserve ignored"
    );
    std::fs::remove_file(created.path.join("ignored-file")).unwrap();
    #[cfg(unix)]
    {
        std::fs::write(
            created.path.join(".slow-close-test"),
            "test-only supervisor delay",
        )
        .unwrap();
        let slow = Uuid::new_v4();
        terminal::execute(
            &service,
            request(Action::Open {
                request_id: slow,
                rows: 24,
                cols: 80,
            }),
        )
        .await
        .unwrap();
        for _ in 0..100 {
            if created.path.join("slow-child.pid").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let slow_pid: i32 = std::fs::read_to_string(created.path.join("slow-child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let start = std::time::Instant::now();
        terminal::execute(&service, request(Action::Close { session_id: slow }))
            .await
            .unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(500),
            "must not SIGKILL slow ownership supervisor after 200ms"
        );
        assert!(
            !Path::new(&format!("/proc/{slow_pid}")).exists(),
            "slow supervisor must finish descendant cleanup"
        );
        std::fs::remove_file(created.path.join(".slow-close-test")).unwrap();
        std::fs::remove_file(created.path.join("slow-child.pid")).unwrap();
    }
    let mut unconfirmed = remove();
    unconfirmed.external_sessions_checked = false;
    assert_eq!(
        remove_worktree::remove(&service, unconfirmed)
            .await
            .unwrap_err()
            .0,
        "remove_confirmation"
    );
    git(&repo, &["worktree", "lock", created.path.to_str().unwrap()]);
    assert_eq!(
        remove_worktree::remove(&service, remove())
            .await
            .unwrap_err()
            .0,
        "remove_protected"
    );
    git(
        &repo,
        &["worktree", "unlock", created.path.to_str().unwrap()],
    );
    let removed = remove_worktree::remove(&service, remove()).await.unwrap();
    assert!(removed.git_removed);
    assert_eq!(removed.path_exists, Some(false));
    assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
    let primary = service
        .worktrees
        .refresh(project.id)
        .await
        .unwrap()
        .worktrees
        .into_iter()
        .find(|t| t.is_main)
        .unwrap();
    let primary_remove = remove_worktree::Request {
        missing_only: false,
        project_id: project.id,
        worktree_id: primary.id,
        confirm_path: primary.path.to_str().unwrap().into(),
        external_sessions_checked: true,
    };
    assert_eq!(
        remove_worktree::remove(&service, primary_remove)
            .await
            .unwrap_err()
            .0,
        "remove_protected"
    );
    service.terminals.close_all();
    println!("PASS PTY open/idempotency/scope/input/Unicode/cwd/resize/bounded-output/exit-code/close; removal active/dirty/confirmation/locked/primary/clean; unchanged source index");
}

#[cfg(unix)]
fn slow_supervisor() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn stop(_: i32) {
        STOP.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGHUP, stop as *const () as usize);
    }
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    std::fs::write("slow-child.pid", child.id().to_string()).unwrap();
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(600));
    child.kill().unwrap();
    child.wait().unwrap();
}
