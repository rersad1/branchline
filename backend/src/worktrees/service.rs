use super::parser::{parse_worktrees, WorktreeRecord};
use crate::{
    config::Project,
    git::{GitCommand, GitError, GitRunner, RepoQuery},
    projects::{Registry, RegistryError},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SnapshotError {
    pub code: String,
    pub message: String,
}

impl SnapshotError {
    fn unavailable() -> Self {
        Self {
            code: "worktree_unavailable".into(),
            message: "Worktree identity or directory could not be verified".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GitWorktree {
    pub id: Uuid,
    pub project_id: Uuid,
    pub environment_id: Uuid,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
    pub is_prunable: bool,
    pub prune_reason: Option<String>,
    pub is_available: bool,
    pub availability_error: Option<SnapshotError>,
    pub git_status: crate::git::status::GitStatus,
    #[serde(skip)]
    fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub project_id: Uuid,
    pub environment_id: Uuid,
    pub worktrees: Vec<GitWorktree>,
    pub last_successful_refresh_at: Option<DateTime<Utc>>,
    pub error: Option<SnapshotError>,
}

struct Inner {
    registry: Registry,
    git: GitRunner,
    entries: RwLock<HashMap<Uuid, Arc<Mutex<Snapshot>>>>,
}

#[derive(Clone)]
pub struct WorktreeService(Arc<Inner>);

impl WorktreeService {
    pub fn new(registry: Registry) -> Self {
        Self(Arc::new(Inner {
            git: registry.git_runner(),
            registry,
            entries: RwLock::new(HashMap::new()),
        }))
    }

    async fn entry(&self, id: Uuid) -> Arc<Mutex<Snapshot>> {
        self.0
            .entries
            .write()
            .await
            .entry(id)
            .or_insert_with(|| {
                Arc::new(Mutex::new(Snapshot {
                    project_id: id,
                    environment_id: self.0.registry.environment().id,
                    worktrees: Vec::new(),
                    last_successful_refresh_at: None,
                    error: None,
                }))
            })
            .clone()
    }

    pub async fn list(&self, id: Uuid) -> Result<Snapshot, RegistryError> {
        self.0.registry.get(id).await?;
        if let Some(entry) = self.0.entries.read().await.get(&id).cloned() {
            let snapshot = entry.lock().await.clone();
            if snapshot.last_successful_refresh_at.is_some() || snapshot.error.is_some() {
                return Ok(snapshot);
            }
        }
        self.refresh(id).await
    }

    pub async fn refresh(&self, id: Uuid) -> Result<Snapshot, RegistryError> {
        let project = self.0.registry.get(id).await?;
        let entry = self.entry(id).await;
        let mut snapshot = entry.lock().await;
        match self.discover(&project, &snapshot.worktrees).await {
            Ok(worktrees) => {
                snapshot.worktrees = worktrees;
                snapshot.last_successful_refresh_at = Some(Utc::now());
                snapshot.error = None;
            }
            Err(error) => {
                snapshot.error = Some(error.clone());
                for tree in &mut snapshot.worktrees {
                    tree.is_available = false;
                    tree.availability_error = Some(error.clone());
                    tree.git_status.error = Some(error.clone());
                }
            }
        }
        Ok(snapshot.clone())
    }

    async fn discover(
        &self,
        project: &Project,
        previous: &[GitWorktree],
    ) -> Result<Vec<GitWorktree>, SnapshotError> {
        let directory = canonical(&project.git_common_dir)
            .await
            .ok_or_else(SnapshotError::unavailable)?;
        if directory != project.git_common_dir {
            return Err(SnapshotError::unavailable());
        }
        // Preserve an unavailable executable separately from an unverifiable identity.
        let bytes = self
            .0
            .git
            .run(&project.git_common_dir, GitCommand::Query(RepoQuery::CommonDir))
            .await
            .map_err(|error| match error {
                GitError::Spawn => SnapshotError {
                    code: "git_unavailable".into(),
                    message: if cfg!(windows) {
                        "Установите Git for Windows в PATH и перезапустите приложение."
                    } else {
                        "Установите Linux Git в выбранной среде (/usr/bin/git) и повторите подключение."
                    }
                    .into(),
                },
                _ => SnapshotError::unavailable(),
            })?;
        let common = canonical_query(&bytes)
            .await
            .ok_or_else(SnapshotError::unavailable)?;
        if common != project.git_common_dir {
            return Err(SnapshotError::unavailable());
        }
        let bytes = self
            .0
            .git
            .run(&common, GitCommand::Worktrees)
            .await
            .map_err(|error| SnapshotError {
                code: "git_refresh_failed".into(),
                message: error.to_string(),
            })?;
        let records = parse_worktrees(&bytes).map_err(|error| SnapshotError {
            code: "invalid_worktree_snapshot".into(),
            message: error.to_string(),
        })?;
        let mut worktrees = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (index, record) in records.into_iter().enumerate() {
            let mut tree = self
                .resolve_record(project, record, index == 0, previous)
                .await;
            self.refresh_status(&mut tree).await;
            if !seen.insert(tree.path.clone()) {
                return Err(SnapshotError {
                    code: "ambiguous_worktree_path".into(),
                    message: "Multiple records resolve to the same canonical path".into(),
                });
            }
            worktrees.push(tree);
        }
        Ok(worktrees)
    }

    async fn resolve_record(
        &self,
        project: &Project,
        record: WorktreeRecord,
        is_main: bool,
        previous: &[GitWorktree],
    ) -> GitWorktree {
        let canonical = canonical(&record.path).await;
        let path = canonical.clone().unwrap_or_else(|| record.path.clone());
        let root = if record.is_bare {
            self.query_path(&path, RepoQuery::GitDir).await
        } else {
            self.query_path(&path, RepoQuery::Root).await
        };
        let common = self.query_path(&path, RepoQuery::CommonDir).await;
        let admin = self.query_path(&path, RepoQuery::GitDir).await;
        let valid = canonical.is_some()
            && root.as_ref() == Some(&path)
            && common.as_ref() == Some(&project.git_common_dir);
        let identity = if valid {
            match admin {
                Some(admin) => fingerprint(admin, path.clone()).await,
                None => None,
            }
        } else {
            None
        };
        let previous = previous.iter().find(|tree| tree.path == path);
        let remembered = identity
            .clone()
            .or_else(|| previous.and_then(|tree| tree.fingerprint.clone()));
        let key = format!(
            "{}\0{}\0{}",
            self.0.registry.environment().id,
            path.display(),
            remembered.as_deref().unwrap_or("unverified")
        );
        let id = Uuid::new_v5(&project.id, key.as_bytes());
        let git_status = previous
            .filter(|tree| tree.id == id)
            .map(|tree| tree.git_status.clone())
            .unwrap_or_default();
        GitWorktree {
            id,
            project_id: project.id,
            environment_id: self.0.registry.environment().id,
            path,
            branch: record.branch,
            head: record.head,
            is_main,
            is_bare: record.is_bare,
            is_detached: record.is_detached,
            is_locked: record.is_locked,
            lock_reason: record.lock_reason,
            is_prunable: record.is_prunable,
            prune_reason: record.prune_reason,
            is_available: identity.is_some() && !record.is_prunable,
            availability_error: if identity.is_some() && !record.is_prunable {
                None
            } else {
                Some(SnapshotError::unavailable())
            },
            fingerprint: remembered,
            git_status,
        }
    }

    async fn query_path(&self, path: &Path, query: RepoQuery) -> Option<PathBuf> {
        let bytes = self.0.git.run(path, GitCommand::Query(query)).await.ok()?;
        canonical_query(&bytes).await
    }

    async fn refresh_status(&self, tree: &mut GitWorktree) {
        use crate::git::status::parse_status;
        if !tree.is_available {
            tree.git_status.error = tree.availability_error.clone();
            return;
        }
        if tree.is_bare {
            tree.git_status.error = Some(SnapshotError {
                code: "bare_repository".into(),
                message: "Bare repository has no working tree status".into(),
            });
            return;
        }
        let result = self.0.git.run(&tree.path, GitCommand::Status).await;
        let current = match self.query_path(&tree.path, RepoQuery::GitDir).await {
            Some(admin) => fingerprint(admin, tree.path.clone()).await,
            None => None,
        };
        let root = self.query_path(&tree.path, RepoQuery::Root).await;
        if current != tree.fingerprint || root.as_ref() != Some(&tree.path) {
            tree.is_available = false;
            tree.availability_error = Some(SnapshotError::unavailable());
            tree.git_status.error = tree.availability_error.clone();
            return;
        }
        match result {
            Ok(bytes) => match parse_status(&bytes) {
                Ok(status) => tree.git_status = status,
                Err(error) => {
                    tree.git_status.error = Some(SnapshotError {
                        code: "invalid_git_status".into(),
                        message: error.to_string(),
                    })
                }
            },
            Err(error) => {
                tree.git_status.error = Some(SnapshotError {
                    code: "git_status_failed".into(),
                    message: error.to_string(),
                })
            }
        }
    }

    pub async fn status(&self, id: Uuid) -> Result<crate::git::status::GitStatus, RegistryError> {
        Ok(self.get(id).await?.git_status)
    }

    pub async fn get(&self, id: Uuid) -> Result<GitWorktree, RegistryError> {
        let mut project_id = self.find_project(id).await;
        if project_id.is_none() {
            self.refresh_all().await?;
            project_id = self.find_project(id).await;
        }
        let project_id = project_id.ok_or(RegistryError::NotFound)?;
        self.refresh(project_id)
            .await?
            .worktrees
            .into_iter()
            .find(|tree| tree.id == id)
            .ok_or(RegistryError::NotFound)
    }

    async fn find_project(&self, id: Uuid) -> Option<Uuid> {
        let entries: Vec<_> = self.0.entries.read().await.values().cloned().collect();
        for entry in entries {
            let snapshot = entry.lock().await;
            if snapshot.worktrees.iter().any(|tree| tree.id == id) {
                return Some(snapshot.project_id);
            }
        }
        None
    }

    pub async fn refresh_all(&self) -> Result<(), RegistryError> {
        let projects = self.0.registry.list().await?;
        let ids: std::collections::HashSet<_> = projects.iter().map(|project| project.id).collect();
        self.0
            .entries
            .write()
            .await
            .retain(|id, _| ids.contains(id));
        for batch in projects.chunks(4) {
            let mut tasks = tokio::task::JoinSet::new();
            for project in batch {
                let service = self.clone();
                let id = project.id;
                tasks.spawn(async move { service.refresh(id).await });
            }
            while let Some(result) = tasks.join_next().await {
                if !matches!(result, Ok(Ok(_))) {
                    tracing::warn!("project refresh failed; other projects continue");
                }
            }
        }
        Ok(())
    }

    pub async fn run_refresh_loop(&self, period: Duration) {
        let mut timer = tokio::time::interval(period);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if let Err(error) = self.refresh_all().await {
                tracing::warn!(%error, "registry refresh failed");
            }
        }
    }
}

async fn canonical_query(bytes: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
    if !Path::new(text).is_absolute() {
        return None;
    }
    canonical(Path::new(text)).await
}

async fn canonical(path: &Path) -> Option<PathBuf> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let path = crate::environment::canonical_path(&path).ok()?;
        if path.to_str().is_none() || !path.is_dir() {
            return None;
        }
        std::fs::read_dir(&path).ok()?;
        Some(path)
    })
    .await
    .ok()?
}

async fn fingerprint(path: PathBuf, root: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path).ok()?;
        let root_metadata = std::fs::metadata(&root).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(format!(
                "{}\0{}:{}:{:?}\0{}:{}:{:?}",
                path.display(),
                metadata.dev(),
                metadata.ino(),
                metadata.created().ok(),
                root_metadata.dev(),
                root_metadata.ino(),
                root_metadata.created().ok()
            ))
        }
        #[cfg(windows)]
        {
            let _ = (metadata, root_metadata);
            Some(format!(
                "{}\0{}",
                crate::git::windows::file_identity(&path)?,
                crate::git::windows::file_identity(&root)?
            ))
        }
    })
    .await
    .ok()?
}
