use super::*;
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};
async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn assert_dead(path: &Path) {
    let pids = std::fs::read_to_string(path).unwrap();
    for pid in pids.split_whitespace() {
        let pid = pid.parse().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // SAFETY: read-only synchronization on recorded owned test processes.
                let dead = unsafe {
                    let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
                    if handle.is_null() {
                        true
                    } else {
                        let dead = WaitForSingleObject(handle, 0) == 0;
                        CloseHandle(handle);
                        dead
                    }
                };
                if dead {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn job_timeout_abort_bounds_and_error_isolation() {
    let directory = tempfile::tempdir().unwrap();
    let exe = directory.path().join("git-fixture.exe");
    let status = std::process::Command::new("rustc")
        .arg("--edition=2021")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_tree.rs"))
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(status.success());
    let mut runner = GitRunner::new(1, Duration::from_secs(2), 1024);
    runner.executable = exe;
    assert!(matches!(
        runner.run(directory.path(), GitCommand::Worktrees).await,
        Err(GitError::Timeout)
    ));
    assert_dead(&directory.path().join("pids")).await;
    std::fs::remove_file(directory.path().join("pids")).unwrap();
    let mut long = runner.clone();
    long.timeout = Duration::from_secs(60);
    let path = directory.path().to_owned();
    let task = tokio::spawn(async move { long.run(&path, GitCommand::Worktrees).await });
    wait_file(&directory.path().join("pids")).await;
    // A queued request must time out without starting another child.
    let mut queued = runner.clone();
    queued.timeout = Duration::from_millis(100);
    assert!(matches!(
        queued.run(directory.path(), GitCommand::Status).await,
        Err(GitError::Timeout)
    ));
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_dead(&directory.path().join("pids")).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while runner.permits.available_permits() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    for name in ["bad", "large", "ok"] {
        std::fs::create_dir(directory.path().join(name)).unwrap();
    }
    assert!(matches!(
        runner
            .run(&directory.path().join("bad"), GitCommand::Worktrees)
            .await,
        Err(GitError::Exit(Some(17)))
    ));
    assert!(matches!(
        runner
            .run(&directory.path().join("large"), GitCommand::Worktrees)
            .await,
        Err(GitError::OutputLimit)
    ));
    let ok = directory.path().join("ok");
    let output = runner.run(&ok, GitCommand::Status).await.unwrap();
    assert_eq!(
        std::str::from_utf8(&output).unwrap().trim(),
        ok.to_str().unwrap()
    );
}
