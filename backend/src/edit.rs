//! Full working-file editing. No Git mutation; optimistic byte/identity checks.
pub(crate) mod file;
use crate::{git::GitCommand, service::WorkspaceService};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const LIMIT: usize = 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub path: String,
    pub version: Option<String>,
    pub text: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub path: String,
    pub version: String,
    pub text: String,
    pub original: String,
}
#[derive(Debug, thiserror::Error)]
#[error("{}", message(.0))]
pub struct Error(pub &'static str);
pub fn message(code: &str) -> &'static str {
    match code {
        "edit_conflict" => "Файл изменился снаружи. Черновик сохранён в Changes; перечитайте внешний файл перед разрешением конфликта.",
        "edit_encoding" => "Только чтение: поддерживается UTF-8 с BOM и без BOM; бинарная или другая кодировка не преобразуется.",
        "edit_large" => "Только чтение: полный файл превышает 1 MiB.",
        "edit_readonly" => "Только чтение: удалённый, конфликтный, вложенный или нетекстовый файл.",
        "edit_path" => "Запись запрещена: путь, symlink/junction, hardlink или Git metadata не прошли проверку.",
        "edit_scope" => "Worktree недоступен или его identity изменилась. Черновик сохранён.",
        _ => "Не удалось сохранить файл. Проверьте права и свободное место; черновик сохранён. Повторите сохранение.",
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self("edit_io")
    }
}

fn decode(bytes: &[u8]) -> Result<(String, bool), Error> {
    let bom = bytes.starts_with(&[239, 187, 191]);
    let text = std::str::from_utf8(if bom { &bytes[3..] } else { bytes })
        .map_err(|_| Error("edit_encoding"))?;
    if text.contains('\0') {
        return Err(Error("edit_encoding"));
    }
    Ok((text.into(), bom))
}
fn encode(text: &str, bom: bool) -> Result<Vec<u8>, Error> {
    if text.contains('\0') {
        return Err(Error("edit_encoding"));
    }
    let mut bytes = if bom { vec![239, 187, 191] } else { vec![] };
    bytes.extend_from_slice(text.as_bytes());
    if bytes.len() > LIMIT {
        return Err(Error("edit_large"));
    }
    Ok(bytes)
}
pub async fn execute(service: &WorkspaceService, request: Request) -> Result<Response, Error> {
    // Shares the mutation gate with registration/removal. Never cancel an active file write.
    let guard = service.create_lock.clone().lock_owned().await;
    if !crate::diff::safe_path(&request.path)
        || request.path.split('/').any(|p| {
            p.contains([':', '\\']) || p.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".git")
        })
    {
        return Err(Error("edit_path"));
    }
    if request.text.is_some() != request.version.is_some() {
        return Err(Error("edit_scope"));
    }
    let snapshot = service
        .worktrees
        .refresh(request.project_id)
        .await
        .map_err(|_| Error("edit_scope"))?;
    let tree = snapshot
        .worktrees
        .iter()
        .find(|t| {
            t.id == request.worktree_id
                && t.is_available
                && !t.is_bare
                && t.availability_error.is_none()
        })
        .ok_or(Error("edit_scope"))?;
    if snapshot.error.is_some() || tree.git_status.error.is_some() {
        return Err(Error("edit_scope"));
    }
    let root = tree.path.clone();
    let git = service.registry.git_runner();
    let mut metadata_paths = Vec::new();
    for query in [
        crate::git::RepoQuery::GitDir,
        crate::git::RepoQuery::CommonDir,
    ] {
        let bytes = git
            .run(&root, GitCommand::Query(query))
            .await
            .map_err(|_| Error("edit_scope"))?;
        let path = std::str::from_utf8(&bytes)
            .map_err(|_| Error("edit_scope"))?
            .strip_suffix('\n')
            .ok_or(Error("edit_scope"))?;
        metadata_paths.push(
            crate::environment::canonical_path(std::path::Path::new(path))
                .map_err(|_| Error("edit_scope"))?,
        );
    }
    let target = crate::environment::canonical_path(&root.join(&request.path))
        .map_err(|_| Error("edit_path"))?;
    if !target.starts_with(&root) || metadata_paths.iter().any(|p| target.starts_with(p)) {
        return Err(Error("edit_path"));
    }
    if let Some(f) = tree
        .git_status
        .files
        .iter()
        .find(|f| f.path.to_str() == Some(&request.path))
    {
        if f.kind == crate::git::status::FileKind::Unmerged || f.submodule.is_some() {
            return Err(Error("edit_readonly"));
        }
    }
    // Read the whole index blob through environment-local Git, never through a pathspec/shell.
    let original = match service
        .registry
        .git_runner()
        .run(&root, GitCommand::ReadIndex(request.path.clone()))
        .await
    {
        Ok(b) => {
            if b.len() > LIMIT {
                return Err(Error("edit_large"));
            }
            decode(&b)?.0
        }
        Err(crate::git::GitError::Exit(_)) => String::new(),
        Err(_) => return Err(Error("edit_scope")),
    };
    let environment_id = service.environment.id;
    let scope = Uuid::new_v5(
        &environment_id,
        format!(
            "{}:{}:{}",
            request.project_id, request.worktree_id, request.path
        )
        .as_bytes(),
    );
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let mut opened = file::open(&root, &request.path, request.text.is_some())?;
        let before = opened.read()?;
        let (text, bom) = decode(&before)?;
        let version_of = |opened: &file::Opened, bytes: &[u8]| {
            Uuid::new_v5(&scope, opened.version(bytes).as_bytes()).to_string()
        };
        let version = version_of(&opened, &before);
        let (text, version) = if let Some(next) = request.text {
            let bytes = encode(&next, bom)?;
            if request.version.as_deref() != Some(&version) {
                return Err(Error("edit_conflict"));
            }
            opened.write(&bytes, &before)?;
            (next, version_of(&opened, &bytes))
        } else {
            (text, version)
        };
        Ok(Response {
            environment_id,
            project_id: request.project_id,
            worktree_id: request.worktree_id,
            path: request.path,
            version,
            text,
            original,
        })
    })
    .await
    .map_err(|_| Error("edit_io"))?
}
