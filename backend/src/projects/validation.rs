use crate::{
    config::Project,
    git::{GitCommand, GitError, GitRunner, RepoQuery},
};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("An absolute environment-local directory path is required")]
    InvalidPath,
    #[error("Project directory is unavailable")]
    Unavailable,
    #[error("Directory is not a supported Git repository")]
    NotRepository,
    #[error("Git could not be started")]
    GitUnavailable,
    #[error("Git validation timed out")]
    GitTimeout,
    #[error("Git returned an unsupported path")]
    UnsupportedPath,
    #[error("Project already registered")]
    Duplicate,
}

#[derive(Debug, PartialEq)]
pub struct ResolvedRepository {
    pub root_path: PathBuf,
    pub git_common_dir: PathBuf,
}

pub async fn validate_registration(
    path: &str,
    existing: &[Project],
) -> Result<ResolvedRepository, ProjectError> {
    validate_registration_with(&GitRunner::default(), path, existing).await
}

pub(super) async fn validate_registration_with(
    runner: &GitRunner,
    path: &str,
    existing: &[Project],
) -> Result<ResolvedRepository, ProjectError> {
    if !crate::environment::valid_path(path) {
        return Err(ProjectError::InvalidPath);
    }
    let path = PathBuf::from(path);
    let directory = tokio::task::spawn_blocking(move || {
        let canonical =
            crate::environment::canonical_path(&path).map_err(|_| ProjectError::Unavailable)?;
        #[cfg(windows)]
        if !crate::environment::supported_git_directory(&canonical) {
            return Err(ProjectError::UnsupportedPath);
        }
        if !canonical.is_dir() {
            return Err(ProjectError::Unavailable);
        }
        std::fs::read_dir(&canonical).map_err(|_| ProjectError::Unavailable)?;
        Ok(canonical)
    })
    .await
    .map_err(|_| ProjectError::Unavailable)??;
    let bare = git_value(runner, &directory, RepoQuery::Bare).await?;
    let common_dir = git_value(runner, &directory, RepoQuery::CommonDir).await?;
    let root = if bare == "true" {
        common_dir.clone()
    } else {
        git_value(runner, &directory, RepoQuery::Root).await?
    };
    let resolved = tokio::task::spawn_blocking(move || {
        let root_path = canonical_git_path(&root)?;
        let git_common_dir = canonical_git_path(&common_dir)?;
        Ok::<_, ProjectError>(ResolvedRepository {
            root_path,
            git_common_dir,
        })
    })
    .await
    .map_err(|_| ProjectError::Unavailable)??;
    if existing
        .iter()
        .any(|project| project.git_common_dir == resolved.git_common_dir)
    {
        return Err(ProjectError::Duplicate);
    }
    Ok(resolved)
}

fn canonical_git_path(value: &str) -> Result<PathBuf, ProjectError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ProjectError::UnsupportedPath);
    }
    let canonical =
        crate::environment::canonical_path(path).map_err(|_| ProjectError::Unavailable)?;
    if canonical.to_str().is_none() {
        return Err(ProjectError::UnsupportedPath);
    }
    Ok(canonical)
}

async fn git_value(
    runner: &GitRunner,
    directory: &Path,
    query: RepoQuery,
) -> Result<String, ProjectError> {
    let bytes = runner
        .run(directory, GitCommand::Query(query))
        .await
        .map_err(|error| match error {
            GitError::Timeout => ProjectError::GitTimeout,
            GitError::Spawn | GitError::Io => ProjectError::GitUnavailable,
            GitError::Exit(_) => ProjectError::NotRepository,
            GitError::OutputLimit => ProjectError::UnsupportedPath,
        })?;
    let value = String::from_utf8(bytes).map_err(|_| ProjectError::UnsupportedPath)?;
    // Strip exactly Git's output terminator, preserving newlines in Linux paths.
    value
        .strip_suffix('\n')
        .map(str::to_owned)
        .ok_or(ProjectError::UnsupportedPath)
}
