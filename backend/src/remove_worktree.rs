//! Conservative Git-owned removal. Unknown external sessions require explicit human check.
use crate::{
    git::{GitCommand, GitError},
    service::WorkspaceService,
    worktrees::Snapshot,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub confirm_path: String,
    pub external_sessions_checked: bool,
    #[serde(default)]
    pub missing_only: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub git_removed: bool,
    pub path_exists: Option<bool>,
    pub snapshot: Option<Snapshot>,
    pub warning: Option<String>,
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RemoveError(pub &'static str);
pub fn message(code: &str) -> &'static str {
    match code {
        "remove_scope" => "Worktree недоступен или его canonical identity изменилась.",
        "remove_protected" => "Primary/bare/locked worktree удалять нельзя. Недоступную запись можно удалить только при отсутствии папки.",
        "remove_dirty" => {
            "Удаление заблокировано: dirty/untracked/conflicts либо Git status не подтверждён."
        }
        "remove_session" => {
            "Закройте управляемые терминалы и вручную проверьте внешние сессии перед удалением."
        }
        "remove_confirmation" => "Подтвердите полный путь worktree и проверку внешних сессий.",
        _ => "Удаление Git не подтверждено. Проверьте фактический Explorer и путь.",
    }
}
pub async fn remove(service: &WorkspaceService, request: Request) -> Result<Response, RemoveError> {
    let _write = service.create_lock.lock().await;
    let snapshot = service
        .worktrees
        .refresh(request.project_id)
        .await
        .map_err(|_| RemoveError("remove_scope"))?;
    let tree = snapshot
        .worktrees
        .iter()
        .find(|t| t.id == request.worktree_id)
        .ok_or(RemoveError("remove_scope"))?;
    if snapshot.error.is_some() {
        return Err(RemoveError("remove_scope"));
    }
    if tree.is_main || tree.is_bare || tree.is_locked {
        return Err(RemoveError("remove_protected"));
    }
    // Missing mode is explicit and still uses Git's single-path, non-force remove.
    // Never treat permission errors, symlinks or an existing replacement as missing.
    if request.missing_only {
        if !tree.is_prunable
            || tree.is_available
            || !matches!(std::fs::symlink_metadata(&tree.path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            return Err(RemoveError("remove_scope"));
        }
    } else {
        if !tree.is_available || tree.availability_error.is_some() {
            return Err(RemoveError("remove_scope"));
        }
        if tree.is_prunable {
            return Err(RemoveError("remove_protected"));
        }
        let status = &tree.git_status;
        if status.error.is_some()
            || status.last_successful_refresh_at.is_none()
            || !status.files.is_empty()
            || [
                status.staged,
                status.unstaged,
                status.untracked,
                status.conflicts,
            ]
            .iter()
            .any(|c| *c != Some(0))
        {
            return Err(RemoveError("remove_dirty"));
        }
        // Ignored files are user data too; ordinary Explorer status intentionally omits them.
        if !service
            .registry
            .git_runner()
            .run(&tree.path, GitCommand::RemovalStatus)
            .await
            .map_err(|_| RemoveError("remove_dirty"))?
            .is_empty()
        {
            return Err(RemoveError("remove_dirty"));
        }
    }
    if !request.external_sessions_checked
        || request.confirm_path != tree.path.to_str().ok_or(RemoveError("remove_scope"))?
    {
        return Err(RemoveError("remove_confirmation"));
    }
    if service
        .terminals
        .active(request.project_id, request.worktree_id)
        || service
            .agents
            .active(request.project_id, Some(request.worktree_id))
    {
        return Err(RemoveError("remove_session"));
    }
    service
        .terminals
        .forget(request.project_id, request.worktree_id);
    let project = service
        .registry
        .get(request.project_id)
        .await
        .map_err(|_| RemoveError("remove_scope"))?;
    // Fresh identity/status above; Git checks cleanliness again and refuses unsafe deletion.
    let result = service
        .registry
        .git_runner()
        .run(
            &project.git_common_dir,
            GitCommand::RemoveWorktree(tree.path.clone()),
        )
        .await;
    let path_exists = match std::fs::symlink_metadata(&tree.path) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        _ => None,
    };
    let snapshot = service.worktrees.refresh(request.project_id).await.ok();
    let warning=match &result { Err(GitError::Exit(_))=>Some("Git отказал в удалении; возможны новые изменения или занятость. Повторите проверку, force не используется.".into()),Err(e)=>Some(format!("Удаление не подтверждено: {e}. Проверьте фактический результат; автоматического повтора нет.")),Ok(_) if snapshot.as_ref().is_none_or(|s|s.error.is_some())=>Some("Git удалил worktree, но Explorer refresh не подтверждён.".into()),_=>None };
    Ok(Response {
        environment_id: service.environment.id,
        project_id: request.project_id,
        worktree_id: request.worktree_id,
        git_removed: result.is_ok(),
        path_exists,
        snapshot,
        warning,
    })
}
