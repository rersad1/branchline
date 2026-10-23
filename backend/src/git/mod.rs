pub mod status;
#[cfg(windows)]
pub mod windows;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
};

#[derive(Clone, Copy, Debug)]
pub enum RepoQuery {
    Bare,
    CommonDir,
    Root,
    GitDir,
}

#[derive(Clone, Debug)]
pub enum GitCommand {
    Query(RepoQuery),
    Worktrees,
    RemoveWorktree(PathBuf),
    RemovalStatus,
    CheckBranch(String),
    BranchExists(String),
    CreateWorktree {
        branch: String,
        path: PathBuf,
        base: String,
    },
    Status,
    Head,
    ReadIndex(String),
    ResolveCommit(String),
    MergeBase(String, String),
    Diff {
        range: crate::diff::Range,
        format: crate::diff::Format,
        paths: Vec<PathBuf>,
        context: u16,
    },
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum GitError {
    #[error("Git could not be started")]
    Spawn,
    #[error("Git I/O failed")]
    Io,
    #[error("Git command failed (exit {0:?})")]
    Exit(Option<i32>),
    #[error("Git command timed out")]
    Timeout,
    #[error("Git output limit exceeded")]
    OutputLimit,
}

#[derive(Clone)]
pub struct GitRunner {
    executable: PathBuf,
    permits: Arc<Semaphore>,
    timeout: Duration,
    output_limit: usize,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self::new(4, Duration::from_secs(10), 4 * 1024 * 1024)
    }
}

impl GitRunner {
    pub fn new(concurrency: usize, timeout: Duration, output_limit: usize) -> Self {
        assert!(concurrency > 0 && output_limit > 0 && !timeout.is_zero());
        Self {
            executable: if cfg!(windows) {
                "git.exe"
            } else {
                "/usr/bin/git"
            }
            .into(),
            permits: Arc::new(Semaphore::new(concurrency)),
            timeout,
            output_limit,
        }
    }

    pub async fn run(&self, directory: &Path, operation: GitCommand) -> Result<Vec<u8>, GitError> {
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(self.timeout, self.execute(directory, operation.clone()))
            .await
            .unwrap_or(Err(GitError::Timeout));
        tracing::debug!(
            ?operation,
            elapsed_ms = started.elapsed().as_millis(),
            success = result.is_ok(),
            "Git query finished"
        );
        result
    }

    async fn execute(&self, directory: &Path, operation: GitCommand) -> Result<Vec<u8>, GitError> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| GitError::Io)?;
        let mut command = Command::new(&self.executable);
        if matches!(operation, GitCommand::Diff { .. }) {
            command.arg("--literal-pathspecs");
        }
        #[cfg(windows)]
        command.args(["-c", "core.longpaths=true"]);
        command.args(["-c", "core.fsmonitor=false", "-C"]);
        command.arg(directory);
        match operation {
            GitCommand::Query(query) => {
                let query = match query {
                    RepoQuery::Bare => "--is-bare-repository",
                    RepoQuery::CommonDir => "--git-common-dir",
                    RepoQuery::Root => "--show-toplevel",
                    RepoQuery::GitDir => "--absolute-git-dir",
                };
                command.args(["rev-parse", "--path-format=absolute", query]);
            }
            GitCommand::CheckBranch(branch) => {
                command
                    .arg("check-ref-format")
                    .arg(format!("refs/heads/{branch}"));
            }
            GitCommand::BranchExists(branch) => {
                command
                    .args(["show-ref", "--verify", "--quiet"])
                    .arg(branch);
            }
            GitCommand::CreateWorktree { branch, path, base } => {
                command.args([
                    "-c",
                    if cfg!(windows) {
                        "core.hooksPath=NUL"
                    } else {
                        "core.hooksPath=/dev/null"
                    },
                ]);
                command
                    .args(["worktree", "add", "--no-track", "-b"])
                    .arg(branch)
                    .arg("--")
                    .arg(path)
                    .arg(base);
            }
            GitCommand::RemoveWorktree(path) => {
                command.args(["worktree", "remove", "--"]).arg(path);
            }
            GitCommand::RemovalStatus => {
                command.args([
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--ignored=matching",
                    "--ignore-submodules=none",
                ]);
            }
            GitCommand::Worktrees => {
                command.args(["worktree", "list", "--porcelain", "-z"]);
            }
            GitCommand::Status => {
                command.args([
                    "status",
                    "--porcelain=v2",
                    "-z",
                    "--untracked-files=all",
                    "--ignore-submodules=none",
                ]);
            }
            GitCommand::ReadIndex(path) => {
                command
                    .args(["show", "--no-ext-diff", "--no-textconv"])
                    .arg(format!(":{path}"));
            }
            GitCommand::Head => {
                command.args(["rev-parse", "--verify", "HEAD^{commit}"]);
            }
            GitCommand::ResolveCommit(value) => {
                command
                    .args(["rev-parse", "--verify", "--end-of-options"])
                    .arg(format!("{value}^{{commit}}"));
            }
            GitCommand::MergeBase(base, head) => {
                command.args(["merge-base", "--all"]).arg(base).arg(head);
            }
            GitCommand::Diff {
                range,
                format,
                paths,
                context,
            } => {
                command.args([
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--no-relative",
                    "--find-renames=50%",
                    "--ignore-submodules=none",
                ]);
                match format {
                    crate::diff::Format::Names => {
                        command.args(["--name-status", "-z"]);
                    }
                    crate::diff::Format::Counts => {
                        command.args(["--numstat", "-z"]);
                    }
                    crate::diff::Format::Patch => {
                        command
                            .args(["--patch", "--full-index"])
                            .arg(format!("--unified={context}"));
                    }
                }
                match range {
                    crate::diff::Range::Unstaged => {}
                    crate::diff::Range::Staged => {
                        command.arg("--cached");
                    }
                    crate::diff::Range::Branch(base, head) => {
                        command.arg(base).arg(head);
                    }
                }
                command.arg("--").args(paths);
            }
        }
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_SUSPENDED
                | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
        );
        #[cfg(windows)]
        let job = windows::Job::new()?;
        let child = command.spawn().map_err(|_| GitError::Spawn)?;
        #[cfg(unix)]
        let process_group = child.id();
        let mut guard = ChildGuard {
            child: Some(child),
            permit: Some(permit),
            #[cfg(unix)]
            process_group,
            #[cfg(windows)]
            job: Some(job),
        };
        #[cfg(windows)]
        guard
            .job
            .as_ref()
            .ok_or(GitError::Io)?
            .assign_and_resume(guard.child.as_ref().ok_or(GitError::Io)?)?;
        let child = guard.child.as_mut().ok_or(GitError::Io)?;
        let stdout = child.stdout.take().ok_or(GitError::Io)?;
        let stderr = child.stderr.take().ok_or(GitError::Io)?;
        let (stdout, _, status) = tokio::try_join!(
            read_bounded(stdout, self.output_limit),
            read_bounded(stderr, 64 * 1024),
            async { child.wait().await.map_err(|_| GitError::Io) },
        )?;
        // Successful wait reaped the child. No asynchronous cleanup is needed.
        guard.child.take();
        if !status.success() {
            return Err(GitError::Exit(status.code()));
        }
        Ok(stdout)
    }
}

async fn read_bounded(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, GitError> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = stream.read(&mut buffer).await.map_err(|_| GitError::Io)?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > limit {
            return Err(GitError::OutputLimit);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

struct ChildGuard {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    #[cfg(unix)]
    process_group: Option<u32>,
    #[cfg(windows)]
    job: Option<windows::Job>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            if let Some(pid) = self.process_group {
                // This child was started in its own process group; cancel descendants too.
                // SAFETY: a positive live child PID is used only as its own group ID.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            let permit = self.permit.take();
            // Keep the concurrency slot until the child has actually been reaped.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                    drop(permit);
                });
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn stub(source: &str, timeout: Duration, limit: usize) -> (tempfile::TempDir, GitRunner) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fake-git");
        std::fs::write(&path, format!("#!/usr/bin/python3\n{source}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut runner = GitRunner::new(1, timeout, limit);
        runner.executable = path;
        (directory, runner)
    }

    async fn wait_file(path: &Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn assert_terminated(path: &Path) {
        let pids = std::fs::read_to_string(path).unwrap();
        for pid in pids.split_whitespace() {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                    if state.is_err()
                        || state
                            .unwrap()
                            .split(')')
                            .nth(1)
                            .unwrap()
                            .trim_start()
                            .starts_with('Z')
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn timeout_and_abort_terminate_process_group_and_release_slot() {
        let source = "import os,sys,time,subprocess\nfrom pathlib import Path\np=Path(sys.argv[4])\nchild=subprocess.Popen(['sleep','30'])\n(p/'pids').write_text(str(os.getpid())+' '+str(child.pid))\ntime.sleep(30)";
        let (directory, runner) = stub(source, Duration::from_millis(200), 1024);
        assert!(matches!(
            runner.run(directory.path(), GitCommand::Worktrees).await,
            Err(GitError::Timeout)
        ));
        assert_terminated(&directory.path().join("pids")).await;
        std::fs::remove_file(directory.path().join("pids")).unwrap();
        let mut long = runner.clone();
        long.timeout = Duration::from_secs(30);
        let path = directory.path().to_owned();
        let task = tokio::spawn(async move { long.run(&path, GitCommand::Worktrees).await });
        wait_file(&directory.path().join("pids")).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_terminated(&directory.path().join("pids")).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while runner.permits.available_permits() != 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn concurrency_and_arguments_are_bounded_without_shell() {
        let source = "import sys,time,json\nfrom pathlib import Path\np=Path(sys.argv[4])\nwith (p/'starts').open('a') as f: f.write('started\\n')\ntime.sleep(0.1)\nprint(json.dumps(sys.argv[1:]))";
        let (directory, runner) = stub(source, Duration::from_secs(2), 1024);
        let path = directory.path().join("dir ' $(touch SHOULD_NOT_EXIST)\n");
        std::fs::create_dir(&path).unwrap();
        let first_runner = runner.clone();
        let first_path = path.clone();
        let first =
            tokio::spawn(async move { first_runner.run(&first_path, GitCommand::Worktrees).await });
        wait_file(&path.join("starts")).await;
        let second_runner = runner.clone();
        let second_path = path.clone();
        let second =
            tokio::spawn(
                async move { second_runner.run(&second_path, GitCommand::Worktrees).await },
            );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            std::fs::read_to_string(path.join("starts"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        let output = first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        let args: Vec<String> = serde_json::from_slice(&output).unwrap();
        assert_eq!(args[3], path.to_str().unwrap());
        assert_eq!(&args[4..], ["worktree", "list", "--porcelain", "-z"]);
        assert!(!directory.path().join("SHOULD_NOT_EXIST").exists());
    }

    #[tokio::test]
    async fn output_limit_and_one_repository_failure_do_not_poison_runner() {
        let source = "import sys\nfrom pathlib import Path\nif Path(sys.argv[4]).name == 'bad': sys.exit(17)\nprint('ok')";
        let (directory, runner) = stub(source, Duration::from_secs(2), 1024);
        assert!(matches!(
            runner
                .run(&directory.path().join("bad"), GitCommand::Worktrees)
                .await,
            Err(GitError::Exit(Some(17)))
        ));
        assert_eq!(
            runner
                .run(directory.path(), GitCommand::Worktrees)
                .await
                .unwrap(),
            b"ok\n"
        );
        let (directory, runner) = stub("print('x'*8192)", Duration::from_secs(2), 128);
        assert!(matches!(
            runner.run(directory.path(), GitCommand::Worktrees).await,
            Err(GitError::OutputLimit)
        ));
    }
}

#[cfg(all(test, windows))]
mod windows_tests;
