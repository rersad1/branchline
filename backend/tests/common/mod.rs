use std::{
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

pub struct Repositories {
    #[allow(dead_code)] // Owns fixture lifetime even in tests that only query paths.
    pub directory: TempDir,
    pub main: PathBuf,
    #[allow(dead_code)] // Shared fixture: some test binaries only use the main worktree.
    pub linked: PathBuf,
}

pub fn git(path: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    #[cfg(windows)]
    command.args(["-c", "core.longpaths=true"]);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    #[cfg(windows)]
    command.arg("-C").arg(path.canonicalize().unwrap());
    #[cfg(unix)]
    command.arg("-C").arg(path);
    let output = command.args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Repositories {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join(if cfg!(windows) {
            "Проект с пробелом ' $()"
        } else {
            "Проект с пробелом ' $()\n"
        });
        let linked = directory.path().join(if cfg!(windows) {
            "linked дерево"
        } else {
            "linked дерево\n"
        });
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-b", "main"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        git(
            &main,
            &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
        );
        let main = agent_workspace_backend::environment::canonical_path(&main).unwrap();
        let linked = agent_workspace_backend::environment::canonical_path(&linked).unwrap();
        Self {
            directory,
            main,
            linked,
        }
    }
}
