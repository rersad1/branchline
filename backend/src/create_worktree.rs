//! Explicit environment-local write boundary. Never rolls back Git or user files.
use crate::{
    git::{GitCommand, GitError},
    service::WorkspaceService,
    worktrees::Snapshot,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    pub project_id: Uuid,
    pub base_ref: String,
    pub branch: String,
    pub path: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateResponse {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub path: PathBuf,
    pub branch: String,
    pub git_created: bool,
    pub branch_exists: Option<bool>,
    pub path_exists: Option<bool>,
    pub worktree_id: Option<Uuid>,
    pub snapshot: Option<Snapshot>,
    pub warning: Option<String>,
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CreateError(pub &'static str);

pub fn message(code: &str) -> &'static str {
    match code {
        "create_invalid" => "Укажите base ref, новое имя ветки и абсолютный путь выбранной среды.",
        "create_repository" => "Repository недоступен или его canonical identity изменилась.",
        "create_base" => "Base ref недоступен или не указывает на commit.",
        "create_branch" => "Недопустимое имя новой ветки.",
        "create_branch_exists" => "Такая ветка уже существует. Выберите новое имя.",
        "create_path" => "Путь занят, родитель недоступен или путь находится внутри существующего worktree/Git directory. Родитель должен существовать.",
        _ => "Проверка создания не завершена. Проверьте repository и Git выбранной среды.",
    }
}
fn input(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}
fn target(value: &str) -> Result<PathBuf, CreateError> {
    if !crate::environment::valid_path(value) {
        return Err(CreateError("create_invalid"));
    }
    let path = Path::new(value);
    if path.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(CreateError("create_path"));
    }
    let name = path.file_name().ok_or(CreateError("create_path"))?;
    // Refuse Windows aliases (ADS, device names, trimmed names), also on WSL for clarity.
    let leaf = name.to_str().ok_or(CreateError("create_path"))?;
    if leaf.eq_ignore_ascii_case(".git")
        || (cfg!(windows)
            && (leaf.contains(':')
                || leaf.ends_with(['.', ' '])
                || matches!(
                    leaf.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )))
    {
        return Err(CreateError("create_path"));
    }
    let parent =
        crate::environment::canonical_path(path.parent().ok_or(CreateError("create_path"))?)
            .map_err(|_| CreateError("create_path"))?;
    let result = parent.join(name);
    #[cfg(windows)]
    if !crate::environment::supported_git_directory(&result) {
        return Err(CreateError("create_path"));
    }
    match std::fs::symlink_metadata(&result) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(result),
        _ => Err(CreateError("create_path")),
    }
}
pub async fn create(
    service: &WorkspaceService,
    request: CreateRequest,
) -> Result<CreateResponse, CreateError> {
    // Shared across service clones, including HTTP callers. Git additionally owns ref locks.
    let _write = service.create_lock.lock().await;
    if !input(&request.base_ref, 256)
        || !input(&request.branch, 256)
        || request.branch == "HEAD"
        || request.path.len() > 4096
    {
        return Err(CreateError("create_invalid"));
    }
    let project = service
        .registry
        .get(request.project_id)
        .await
        .map_err(|_| CreateError("create_repository"))?;
    let resolved = crate::projects::validate_registration(
        project
            .root_path
            .to_str()
            .ok_or(CreateError("create_repository"))?,
        &[],
    )
    .await
    .map_err(|_| CreateError("create_repository"))?;
    if resolved.git_common_dir != project.git_common_dir {
        return Err(CreateError("create_repository"));
    }
    let git = service.registry.git_runner();
    let directory = &project.git_common_dir;
    git.run(directory, GitCommand::CheckBranch(request.branch.clone()))
        .await
        .map_err(|_| CreateError("create_branch"))?;
    let branch_ref = format!("refs/heads/{}", request.branch);
    match git
        .run(directory, GitCommand::BranchExists(branch_ref.clone()))
        .await
    {
        Ok(_) => return Err(CreateError("create_branch_exists")),
        Err(GitError::Exit(Some(1))) => {}
        Err(_) => return Err(CreateError("create_failed")),
    }
    let base = git
        .run(
            directory,
            GitCommand::ResolveCommit(request.base_ref.clone()),
        )
        .await
        .map_err(|_| CreateError("create_base"))?;
    let base = String::from_utf8(base)
        .map_err(|_| CreateError("create_base"))?
        .trim()
        .to_owned();
    if !matches!(base.len(), 40 | 64) || !base.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CreateError("create_base"));
    }
    let path = target(&request.path)?;
    let snapshot = service
        .worktrees
        .refresh(project.id)
        .await
        .map_err(|_| CreateError("create_repository"))?;
    if snapshot.error.is_some() {
        return Err(CreateError("create_repository"));
    }
    // Worktree creation must never put files into any existing source worktree.
    for tree in &snapshot.worktrees {
        if path.starts_with(&tree.path) {
            return Err(CreateError("create_path"));
        }
    }
    if path.starts_with(directory) {
        return Err(CreateError("create_path"));
    }
    // Exclusive reservation rejects even existing empty directories and dangling symlinks.
    // After this point every failure reports actual observations; no automatic deletion.
    std::fs::create_dir(&path).map_err(|_| CreateError("create_path"))?;
    let result = git
        .run(
            directory,
            GitCommand::CreateWorktree {
                branch: request.branch.clone(),
                path: path.clone(),
                base,
            },
        )
        .await;
    let git_created = result.is_ok();
    let branch_exists = match git
        .run(directory, GitCommand::BranchExists(branch_ref))
        .await
    {
        Ok(_) => Some(true),
        Err(GitError::Exit(Some(1))) => Some(false),
        _ => None,
    };
    let path_exists = match std::fs::symlink_metadata(&path) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        _ => None,
    };
    let snapshot = service.worktrees.refresh(project.id).await.ok();
    let worktree_id = snapshot
        .as_ref()
        .filter(|s| s.error.is_none())
        .and_then(|s| {
            s.worktrees
                .iter()
                .find(|t| t.path == path && t.is_available)
        })
        .map(|t| t.id);
    let warning = if let Err(error) = result {
        Some(format!("Git creation не подтверждено: {error}. Фактические branch/path ниже; файлы не удалялись. Проверьте результат перед повтором."))
    } else if worktree_id.is_none() {
        Some("Git успешно создал worktree, но discovery/selection не подтверждены. Обновите Explorer; повторять создание не нужно.".into())
    } else {
        None
    };
    Ok(CreateResponse {
        environment_id: service.environment.id,
        project_id: project.id,
        path,
        branch: request.branch,
        git_created,
        branch_exists,
        path_exists,
        worktree_id,
        snapshot,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{NewProject, Registry};
    fn git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new(if cfg!(windows) {
            "git.exe"
        } else {
            "/usr/bin/git"
        })
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    async fn fixture() -> (tempfile::TempDir, WorkspaceService, CreateRequest) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("source");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "core.autocrlf", "false"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        std::fs::write(repo.join("tracked.txt"), "dirty source\n").unwrap();
        let service =
            WorkspaceService::new(Registry::open(temp.path().join("config.json")).unwrap());
        let project = service
            .add(NewProject {
                root_path: repo.to_str().unwrap().into(),
                name: None,
                base_ref: None,
            })
            .await
            .unwrap();
        let request = CreateRequest {
            project_id: project.project.id,
            base_ref: "main".into(),
            branch: "feat/create".into(),
            path: temp.path().join("external tree Ю").to_str().unwrap().into(),
        };
        (temp, service, request)
    }
    #[tokio::test]
    async fn create_external_and_collisions_preserve_source() {
        let (temp, service, request) = fixture().await;
        let repo = temp.path().join("source");
        let index = std::fs::read(repo.join(".git/index")).unwrap();
        let response = create(&service, request.clone()).await.unwrap();
        assert!(response.git_created && response.worktree_id.is_some());
        assert_eq!(
            std::fs::read_to_string(Path::new(&request.path).join("tracked.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            create(&service, request.clone()).await.unwrap_err().0,
            "create_branch_exists"
        );
        let mut other = request.clone();
        other.branch = "another".into();
        assert_eq!(
            create(&service, other.clone()).await.unwrap_err().0,
            "create_path"
        );
        other.path = repo.join("nested").to_str().unwrap().into();
        assert_eq!(create(&service, other).await.unwrap_err().0, "create_path");
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
        assert_eq!(
            std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
            "dirty source\n"
        );
    }
    #[tokio::test]
    async fn invalid_base_branch_scope_and_occupied_paths_do_not_write() {
        let (temp, service, request) = fixture().await;
        for (branch, base, code) in [
            ("-bad", "main", "create_invalid"),
            ("bad..name", "main", "create_branch"),
            ("@{-1}", "main", "create_branch"),
            ("valid", "absent", "create_base"),
        ] {
            let mut r = request.clone();
            r.branch = branch.into();
            r.base_ref = base.into();
            assert_eq!(create(&service, r).await.unwrap_err().0, code);
            assert!(!Path::new(&request.path).exists());
        }
        let mut r = request.clone();
        r.project_id = Uuid::new_v4();
        assert_eq!(
            create(&service, r).await.unwrap_err().0,
            "create_repository"
        );
        std::fs::write(&request.path, "preserve").unwrap();
        assert_eq!(
            create(&service, request.clone()).await.unwrap_err().0,
            "create_path"
        );
        assert_eq!(std::fs::read_to_string(&request.path).unwrap(), "preserve");
        let mut r = request.clone();
        r.path = temp.path().join("missing/child").to_str().unwrap().into();
        assert_eq!(create(&service, r).await.unwrap_err().0, "create_path");
        std::fs::rename(
            temp.path().join("source/.git"),
            temp.path().join("moved-git"),
        )
        .unwrap();
        assert_eq!(
            create(&service, request).await.unwrap_err().0,
            "create_repository"
        );
    }
    #[tokio::test]
    async fn partial_checkout_failure_reports_remaining_branch_and_path() {
        let (temp, service, request) = fixture().await;
        let repo = temp.path().join("source");
        std::fs::write(repo.join(".gitattributes"), "tracked.txt filter=broken\n").unwrap();
        git(&repo, &["add", ".gitattributes"]);
        git(&repo, &["commit", "-m", "attribute"]);
        git(
            &repo,
            &["config", "filter.broken.smudge", "awm-nonexistent-filter"],
        );
        git(&repo, &["config", "filter.broken.required", "true"]);
        let index = std::fs::read(repo.join(".git/index")).unwrap();
        let response = create(&service, request.clone()).await.unwrap();
        assert!(!response.git_created);
        assert!(response.warning.is_some());
        assert_eq!(response.branch_exists, Some(true));
        assert_eq!(
            response.path_exists,
            Some(Path::new(&request.path).exists())
        );
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index);
        // Git itself may remove its failed checkout directory; we report that observation.
    }
    #[tokio::test]
    async fn concurrent_duplicate_creates_one_worktree() {
        let (_temp, service, request) = fixture().await;
        let (a, b) = tokio::join!(create(&service, request.clone()), create(&service, request));
        assert!(a.unwrap().git_created);
        assert_eq!(b.unwrap_err().0, "create_branch_exists");
    }
}
