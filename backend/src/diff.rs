//! Read-only, environment/worktree-scoped metadata and selected-file patch.
use crate::{
    git::{
        status::{parse_status, FileKind},
        GitCommand, GitError,
    },
    service::WorkspaceService,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const CONTENT_LIMIT: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub enum Range {
    Unstaged,
    Staged,
    Branch(String, String),
}
#[derive(Clone, Debug)]
pub enum Format {
    Names,
    Counts,
    Patch,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Unstaged,
    Staged,
    Branch,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiffRequest {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub mode: Mode,
    pub base_ref: Option<String>,
    pub file_id: Option<Uuid>,
    #[serde(default = "default_context")]
    pub context_lines: u16,
}
fn default_context() -> u16 {
    3
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffFile {
    pub id: Uuid,
    pub path: String,
    pub original_path: Option<String>,
    pub status: String,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
    pub binary: Option<bool>,
    pub submodule: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileDiff {
    pub file_id: Uuid,
    pub state: String,
    pub patch: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffResponse {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub mode: Mode,
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub state: String,
    pub files: Vec<DiffFile>,
    pub selected_file: Option<FileDiff>,
    pub context_lines: u16,
    pub observed_at: DateTime<Utc>,
}
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("Invalid diff request")]
    Invalid,
    #[error("Worktree is unavailable or its identity changed")]
    Unavailable,
    #[error("Selected file ID is no longer in this scoped diff")]
    FileNotFound,
    #[error("Git diff output limit exceeded")]
    Limit,
    #[error("Git diff request timed out")]
    Timeout,
    #[error("Git diff could not be read")]
    Git,
    #[error("Git diff changed while it was being read; refresh required")]
    Changed,
}
impl DiffError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid => "invalid_diff",
            Self::Unavailable => "worktree_unavailable",
            Self::FileNotFound => "diff_file_not_found",
            Self::Limit => "diff_limit",
            Self::Timeout => "git_timeout",
            Self::Git => "diff_failed",
            Self::Changed => "diff_changed",
        }
    }
}
impl From<GitError> for DiffError {
    fn from(e: GitError) -> Self {
        match e {
            GitError::OutputLimit => Self::Limit,
            GitError::Timeout => Self::Timeout,
            _ => Self::Git,
        }
    }
}
pub(crate) fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && (!cfg!(windows) || !path.contains(['\\', ':']))
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.eq_ignore_ascii_case(".git"))
}
fn path(bytes: &[u8]) -> Result<String, DiffError> {
    let p = std::str::from_utf8(bytes).map_err(|_| DiffError::Invalid)?;
    if !safe_path(p) {
        return Err(DiffError::Invalid);
    }
    Ok(p.into())
}
fn oid(bytes: Vec<u8>) -> Result<String, DiffError> {
    let value = String::from_utf8(bytes)
        .map_err(|_| DiffError::Git)?
        .trim()
        .to_owned();
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DiffError::Git);
    }
    Ok(value)
}
fn names(bytes: &[u8]) -> Result<Vec<DiffFile>, DiffError> {
    if bytes.is_empty() {
        return Ok(vec![]);
    }
    if !bytes.ends_with(b"\0") {
        return Err(DiffError::Git);
    }
    let mut fields = bytes[..bytes.len() - 1].split(|b| *b == 0);
    let mut result = Vec::new();
    while let Some(status) = fields.next() {
        let status = std::str::from_utf8(status).map_err(|_| DiffError::Git)?;
        if !matches!(
            status.as_bytes().first(),
            Some(b'A' | b'M' | b'D' | b'R' | b'C' | b'T' | b'U')
        ) {
            return Err(DiffError::Git);
        }
        let first = path(fields.next().ok_or(DiffError::Git)?)?;
        let (current, original) = if status.starts_with(['R', 'C']) {
            (path(fields.next().ok_or(DiffError::Git)?)?, Some(first))
        } else {
            (first, None)
        };
        result.push(DiffFile {
            id: Uuid::nil(),
            path: current,
            original_path: original,
            status: status[..1].into(),
            additions: None,
            deletions: None,
            binary: None,
            submodule: false,
        });
    }
    // Git can report U plus M/D for the same unmerged path. That path has one
    // scoped identity and must always select the conflict fallback.
    let conflicts: HashSet<_> = result
        .iter()
        .filter(|file| file.status == "U")
        .map(|file| file.path.clone())
        .collect();
    result.retain(|file| file.status == "U" || !conflicts.contains(&file.path));
    let mut seen = HashSet::new();
    if result.iter().any(|file| !seen.insert(&file.path)) {
        return Err(DiffError::Git);
    }
    Ok(result)
}
type LineCounts = HashMap<String, (Option<usize>, Option<usize>)>;
fn counts(bytes: &[u8]) -> Result<LineCounts, DiffError> {
    if bytes.is_empty() {
        return Ok(HashMap::new());
    }
    if !bytes.ends_with(b"\0") {
        return Err(DiffError::Git);
    }
    let mut fields = bytes[..bytes.len() - 1].split(|b| *b == 0);
    let mut result = HashMap::new();
    while let Some(record) = fields.next() {
        let parts: Vec<_> = record.splitn(3, |b| *b == b'\t').collect();
        if parts.len() != 3 {
            return Err(DiffError::Git);
        }
        let number = |v: &[u8]| -> Result<Option<usize>, DiffError> {
            if v == b"-" {
                Ok(None)
            } else {
                Ok(Some(
                    std::str::from_utf8(v)
                        .map_err(|_| DiffError::Git)?
                        .parse()
                        .map_err(|_| DiffError::Git)?,
                ))
            }
        };
        let p = if parts[2].is_empty() {
            path(fields.next().ok_or(DiffError::Git)?)?;
            path(fields.next().ok_or(DiffError::Git)?)?
        } else {
            path(parts[2])?
        };
        result.insert(p, (number(parts[0])?, number(parts[1])?));
    }
    Ok(result)
}

pub async fn read(
    service: &WorkspaceService,
    request: DiffRequest,
) -> Result<DiffResponse, DiffError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        read_inner(service, request),
    )
    .await
    .map_err(|_| DiffError::Timeout)?
}
async fn read_inner(
    service: &WorkspaceService,
    request: DiffRequest,
) -> Result<DiffResponse, DiffError> {
    if request.context_lines > 100
        || request.project_id.is_nil()
        || request.worktree_id.is_nil()
        || request.base_ref.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 256 || s.starts_with('-') || s.chars().any(char::is_control)
        })
        || (request.mode != Mode::Branch && request.base_ref.is_some())
    {
        return Err(DiffError::Invalid);
    }
    let snapshot = service
        .worktrees
        .refresh(request.project_id)
        .await
        .map_err(|_| DiffError::Unavailable)?;
    let tree = snapshot
        .worktrees
        .iter()
        .find(|t| t.id == request.worktree_id)
        .ok_or(DiffError::Unavailable)?;
    if snapshot.error.is_some() || !tree.is_available || tree.availability_error.is_some() {
        return Err(DiffError::Unavailable);
    }
    let mut response = DiffResponse {
        environment_id: service.environment.id,
        project_id: request.project_id,
        worktree_id: request.worktree_id,
        mode: request.mode.clone(),
        base_ref: request.base_ref.clone(),
        base_commit: None,
        head_commit: None,
        state: if tree.is_bare { "bare" } else { "ready" }.into(),
        files: vec![],
        selected_file: None,
        context_lines: request.context_lines,
        observed_at: Utc::now(),
    };
    if tree.is_bare {
        return Ok(response);
    }
    let git = service.registry.git_runner();
    let head = match git.run(&tree.path, GitCommand::Head).await {
        Ok(bytes) => Some(oid(bytes)?),
        Err(GitError::Exit(_))
            if tree
                .head
                .as_ref()
                .is_none_or(|s| s.bytes().all(|b| b == b'0')) =>
        {
            None
        }
        Err(e) => return Err(e.into()),
    };
    response.head_commit = head.clone();
    if head.is_none() {
        response.state = "unborn".into();
    }
    let mut resolved_base = None;
    let range = match request.mode {
        Mode::Unstaged => Range::Unstaged,
        Mode::Staged => Range::Staged,
        Mode::Branch => {
            let Some(head) = head.clone() else {
                return Ok(response);
            };
            let Some(base) = &request.base_ref else {
                response.state = "no_base".into();
                return Ok(response);
            };
            let base = match git
                .run(&tree.path, GitCommand::ResolveCommit(base.clone()))
                .await
            {
                Ok(b) => oid(b)?,
                Err(GitError::Exit(_)) => {
                    response.state = "no_base".into();
                    return Ok(response);
                }
                Err(e) => return Err(e.into()),
            };
            resolved_base = Some(base.clone());
            let base = match git
                .run(&tree.path, GitCommand::MergeBase(base, head.clone()))
                .await
            {
                Ok(b) => match oid(b) {
                    Ok(b) => b,
                    Err(_) => {
                        response.state = "ambiguous_base".into();
                        return Ok(response);
                    }
                },
                Err(GitError::Exit(_)) => {
                    response.state = "no_base".into();
                    return Ok(response);
                }
                Err(e) => return Err(e.into()),
            };
            response.base_commit = Some(base.clone());
            Range::Branch(base, head)
        }
    };
    let metadata = |format| GitCommand::Diff {
        range: range.clone(),
        format,
        paths: vec![],
        context: request.context_lines,
    };
    let names_before = git.run(&tree.path, metadata(Format::Names)).await?;
    let mut files = names(&names_before)?;
    let totals = counts(&git.run(&tree.path, metadata(Format::Counts)).await?)?;
    for file in &mut files {
        let Some((a, d)) = totals.get(&file.path) else {
            return Err(DiffError::Changed);
        };
        if file.status != "U" {
            file.additions = *a;
            file.deletions = *d;
            file.binary = Some(a.is_none() || d.is_none());
        }
        file.submodule = tree
            .git_status
            .files
            .iter()
            .any(|f| f.path.to_str() == Some(&file.path) && f.submodule.is_some());
    }
    let status_before = git.run(&tree.path, GitCommand::Status).await?;
    let status = parse_status(&status_before).map_err(|_| DiffError::Git)?;
    if request.mode == Mode::Unstaged {
        for entry in &status.files {
            if entry.kind == FileKind::Untracked {
                files.push(DiffFile {
                    id: Uuid::nil(),
                    path: path(entry.path.to_str().ok_or(DiffError::Invalid)?.as_bytes())?,
                    original_path: None,
                    status: "?".into(),
                    additions: None,
                    deletions: Some(0),
                    binary: None,
                    submodule: false,
                });
            }
        }
    }
    if request.file_id.is_none() && files.iter().any(|file| file.status == "?") {
        let root = tree.path.clone();
        files = tokio::task::spawn_blocking(move || {
            untracked_counts(&root, &mut files);
            files
        })
        .await
        .map_err(|_| DiffError::Git)?;
    }
    for file in &mut files {
        let identity = format!(
            "{:?}\0{:?}\0{}\0{:?}",
            request.mode, request.base_ref, file.path, file.original_path
        );
        let scope = Uuid::new_v5(&service.environment.id, request.worktree_id.as_bytes());
        file.id = Uuid::new_v5(&scope, identity.as_bytes());
    }
    if let Some(id) = request.file_id {
        let file = files
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or(DiffError::FileNotFound)?;
        let mut selected = FileDiff {
            file_id: id,
            state: "text".into(),
            patch: None,
        };
        if file.binary == Some(true) {
            selected.state = "binary".into();
        } else if file.status == "U" {
            selected.state = "conflict".into();
        } else if file.submodule {
            selected.state = "submodule".into();
        } else if file.status == "?" {
            let root = tree.path.clone();
            let relative = file.path.clone();
            let bytes = tokio::task::spawn_blocking(move || read_untracked(&root, &relative))
                .await
                .map_err(|_| DiffError::Git)??;
            match bytes {
                None => selected.state = "large_file".into(),
                Some(bytes) => {
                    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                        selected.state = "binary".into();
                        file.binary = Some(true);
                    } else {
                        file.binary = Some(false);
                        let text = String::from_utf8(bytes).map_err(|_| DiffError::Git)?;
                        let lines = text.lines().count();
                        file.additions = Some(lines);
                        let a = serde_json::to_string(&format!("a/{}", file.path))
                            .map_err(|_| DiffError::Git)?;
                        let b = serde_json::to_string(&format!("b/{}", file.path))
                            .map_err(|_| DiffError::Git)?;
                        let mut patch = format!("diff --git {a} {b}\n--- /dev/null\n+++ {b}\n");
                        if lines > 0 {
                            patch.push_str(&format!("@@ -0,0 +1,{lines} @@\n"));
                            for line in text.split_inclusive('\n') {
                                patch.push('+');
                                patch.push_str(line);
                                if !line.ends_with('\n') {
                                    patch.push_str("\n\\ No newline at end of file\n");
                                }
                            }
                        }
                        if patch.len() > CONTENT_LIMIT {
                            selected.state = "large_file".into();
                        } else {
                            selected.patch = Some(patch);
                        }
                    }
                }
            }
        } else {
            let paths = file
                .original_path
                .iter()
                .chain(std::iter::once(&file.path))
                .map(PathBuf::from)
                .collect();
            match git
                .run(
                    &tree.path,
                    GitCommand::Diff {
                        range: range.clone(),
                        format: Format::Patch,
                        paths,
                        context: request.context_lines,
                    },
                )
                .await
            {
                Err(GitError::OutputLimit) => selected.state = "large_file".into(),
                Err(e) => return Err(e.into()),
                Ok(bytes) => {
                    if bytes.len() > CONTENT_LIMIT {
                        selected.state = "large_file".into();
                    } else if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                        selected.state = "binary".into();
                    } else {
                        selected.patch =
                            Some(String::from_utf8(bytes).map_err(|_| DiffError::Git)?);
                    }
                }
            }
        }
        response.selected_file = Some(selected);
    }
    let head_after = match git.run(&tree.path, GitCommand::Head).await {
        Ok(b) => Some(oid(b)?),
        Err(GitError::Exit(_)) if head.is_none() => None,
        Err(e) => return Err(e.into()),
    };
    if head_after != head {
        return Err(DiffError::Changed);
    }
    if let (Some(base), Some(expected)) = (&request.base_ref, &resolved_base) {
        if oid(git
            .run(&tree.path, GitCommand::ResolveCommit(base.clone()))
            .await?)?
            != *expected
        {
            return Err(DiffError::Changed);
        }
    }
    if git.run(&tree.path, metadata(Format::Names)).await? != names_before
        || git.run(&tree.path, GitCommand::Status).await? != status_before
    {
        return Err(DiffError::Changed);
    }
    verify_counts(&git, &tree.path, metadata(Format::Counts), &totals).await?;
    response.files = files;
    response.observed_at = Utc::now();
    Ok(response)
}

async fn verify_counts(
    git: &crate::git::GitRunner,
    root: &Path,
    command: GitCommand,
    expected: &LineCounts,
) -> Result<(), DiffError> {
    if counts(&git.run(root, command).await?)? != *expected {
        return Err(DiffError::Changed);
    }
    Ok(())
}

// Metadata never constructs patches. Bound the whole scan as well as each file;
// skipped/unsafe/unreadable files retain unknown counts instead of hiding the list.
fn untracked_counts(root: &Path, files: &mut [DiffFile]) {
    let started = std::time::Instant::now();
    let mut remaining = 64 * CONTENT_LIMIT;
    for file in files
        .iter_mut()
        .filter(|file| file.status == "?")
        .take(4096)
    {
        if remaining < CONTENT_LIMIT + 1 || started.elapsed().as_secs() >= 5 {
            break;
        }
        match read_untracked(root, &file.path) {
            Ok(Some(bytes)) => {
                remaining -= bytes.len();
                if bytes.contains(&0) {
                    file.binary = Some(true);
                } else if let Ok(text) = std::str::from_utf8(&bytes) {
                    file.binary = Some(false);
                    file.additions = Some(text.lines().count());
                } else {
                    file.binary = Some(true);
                }
            }
            Ok(None) | Err(_) => remaining -= CONTENT_LIMIT + 1,
        }
    }
}

fn read_untracked(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, DiffError> {
    // Share the editor's no-follow, ancestor-pinned boundary. Canonical validation
    // alone cannot authorize a later open after a parent is replaced.
    let mut file =
        crate::edit::file::open(root, relative, false).map_err(|_| DiffError::Unavailable)?;
    match file.read() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(crate::edit::Error("edit_large")) => Ok(None),
        Err(_) => Err(DiffError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn untracked_read_rejects_replaced_root_ancestor() {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("parent");
        let outside = fixture.path().join("outside");
        std::fs::create_dir_all(parent.join("root")).unwrap();
        std::fs::create_dir_all(outside.join("root")).unwrap();
        std::fs::write(outside.join("root/secret"), "outside sentinel").unwrap();
        // Simulate substitution after the caller's canonical scope validation.
        let root = parent.join("root");
        std::fs::rename(&parent, fixture.path().join("old-parent")).unwrap();
        std::os::unix::fs::symlink(&outside, &parent).unwrap();
        assert!(read_untracked(&root, "secret").is_err());
        assert_eq!(
            std::fs::read(outside.join("root/secret")).unwrap(),
            b"outside sentinel"
        );
    }

    #[tokio::test]
    async fn changed_modified_content_is_rejected_even_when_names_and_status_match() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git_exe = if cfg!(windows) {
            "git.exe"
        } else {
            "/usr/bin/git"
        };
        for args in [vec!["init"], vec!["add", "tracked"]] {
            if args[0] == "add" {
                std::fs::write(root.join("tracked"), b"index\n").unwrap();
            }
            assert!(std::process::Command::new(git_exe)
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        }
        std::fs::write(root.join("tracked"), b"working\n").unwrap();
        let git = crate::git::GitRunner::default();
        let command = |format| GitCommand::Diff {
            range: Range::Unstaged,
            format,
            paths: vec![],
            context: 3,
        };
        let names_before = git.run(root, command(Format::Names)).await.unwrap();
        let status_before = git.run(root, GitCommand::Status).await.unwrap();
        let totals = counts(&git.run(root, command(Format::Counts)).await.unwrap()).unwrap();
        // The file remains M, but the old text counts must not label a binary patch.
        std::fs::write(root.join("tracked"), b"binary\0content").unwrap();
        assert_eq!(
            git.run(root, command(Format::Names)).await.unwrap(),
            names_before
        );
        assert_eq!(
            git.run(root, GitCommand::Status).await.unwrap(),
            status_before
        );
        assert!(matches!(
            verify_counts(&git, root, command(Format::Counts), &totals).await,
            Err(DiffError::Changed)
        ));
    }
}
