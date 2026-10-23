use super::{validation::validate_registration_with, ProjectError};
use crate::{
    config::{Config, Project},
    git::GitRunner,
    storage::{ConfigStore, StorageError},
};
use chrono::Utc;
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("Project not found")]
    NotFound,
    #[error("Configuration is already in use")]
    Busy,
    #[error("Close managed terminals before removing this registration")]
    ActiveTerminal,
    #[error("Registry worker failed")]
    Worker,
    #[error("{0}")]
    InvalidInput(&'static str),
}

pub struct NewProject {
    pub root_path: String,
    pub name: Option<String>,
    pub base_ref: Option<String>,
}

struct Inner {
    store: ConfigStore,
    config: Mutex<Config>,
    _lock: File,
    git: GitRunner,
    environment: crate::environment::ExecutionEnvironment,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Explicitly unlock: a concurrent fork may briefly inherit this file
        // descriptor before exec closes it, so closing alone can delay release.
        let _ = self._lock.unlock();
    }
}

#[derive(Clone)]
pub struct Registry(Arc<Inner>);

impl Registry {
    pub fn open(path: PathBuf) -> Result<Self, RegistryError> {
        Self::open_in(path, crate::environment::ExecutionEnvironment::local())
    }

    pub fn open_in(
        path: PathBuf,
        environment: crate::environment::ExecutionEnvironment,
    ) -> Result<Self, RegistryError> {
        environment
            .verify_local()
            .map_err(RegistryError::InvalidInput)?;
        let store = ConfigStore::new(path)?;
        let parent = store
            .path()
            .parent()
            .ok_or(StorageError::Invalid("Missing configuration directory"))?;
        fs::create_dir_all(parent).map_err(StorageError::Io)?;
        let path = parent.canonicalize().map_err(StorageError::Io)?.join(
            store
                .path()
                .file_name()
                .ok_or(StorageError::Invalid("Missing configuration filename"))?,
        );
        let store = ConfigStore::new(path)?;
        let mut lock_path = store.path().as_os_str().to_owned();
        lock_path.push(".lock");
        let lock_path = PathBuf::from(lock_path);
        for path in [store.path(), lock_path.as_path()] {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                return Err(StorageError::Invalid(
                    "Configuration and lock files must not be symlinks",
                )
                .into());
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(lock_path).map_err(StorageError::Io)?;
        lock.try_lock().map_err(|_| RegistryError::Busy)?;
        let config = store.load()?;
        store.backup_v1()?;
        Ok(Self(Arc::new(Inner {
            store,
            config: Mutex::new(config),
            _lock: lock,
            git: GitRunner::default(),
            environment,
        })))
    }

    pub fn environment(&self) -> &crate::environment::ExecutionEnvironment {
        &self.0.environment
    }

    pub fn git_runner(&self) -> GitRunner {
        self.0.git.clone()
    }

    async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Inner) -> Result<T, RegistryError> + Send + 'static,
    ) -> Result<T, RegistryError> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let result = operation(&inner);
            // Release the registry owner before publishing the worker result.
            drop(inner);
            result
        })
        .await
        .map_err(|_| RegistryError::Worker)?
    }

    pub async fn list(&self) -> Result<Vec<Project>, RegistryError> {
        self.blocking(|inner| {
            Ok(inner
                .config
                .lock()
                .map_err(|_| RegistryError::Worker)?
                .projects
                .clone())
        })
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Project, RegistryError> {
        self.blocking(move |inner| {
            inner
                .config
                .lock()
                .map_err(|_| RegistryError::Worker)?
                .projects
                .iter()
                .find(|project| project.id == id)
                .cloned()
                .ok_or(RegistryError::NotFound)
        })
        .await
    }

    pub async fn add(&self, request: NewProject) -> Result<Project, RegistryError> {
        if request.name.as_ref().is_some_and(|name| {
            name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control)
        }) {
            return Err(RegistryError::InvalidInput(
                "Project name must contain 1–256 bytes without control characters",
            ));
        }
        if request.base_ref.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
        }) {
            return Err(RegistryError::InvalidInput("Invalid base_ref"));
        }
        let resolved = validate_registration_with(&self.0.git, &request.root_path, &[]).await?;
        let mut name = request.name.unwrap_or_else(|| {
            resolved
                .root_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Project")
                .chars()
                .filter(|character| !character.is_control())
                .take(64)
                .collect::<String>()
        });
        if name.trim().is_empty() {
            name = "Project".into();
        }
        let project = Project {
            id: Uuid::new_v4(),
            name: name.trim().to_owned(),
            root_path: resolved.root_path,
            git_common_dir: resolved.git_common_dir,
            base_ref: request.base_ref,
            created_at: Utc::now(),
        };
        self.blocking(move |inner| {
            let mut current = inner.config.lock().map_err(|_| RegistryError::Worker)?;
            // Recheck inside the transaction so simultaneous main/linked adds cannot both win.
            if current
                .projects
                .iter()
                .any(|entry| entry.git_common_dir == project.git_common_dir)
            {
                return Err(ProjectError::Duplicate.into());
            }
            let mut next = current.clone();
            next.projects.push(project.clone());
            inner.store.save(&next)?;
            *current = next;
            Ok(project)
        })
        .await
    }

    pub async fn remove(&self, id: Uuid) -> Result<(), RegistryError> {
        self.blocking(move |inner| {
            let mut current = inner.config.lock().map_err(|_| RegistryError::Worker)?;
            if !current.projects.iter().any(|project| project.id == id) {
                return Err(RegistryError::NotFound);
            }
            let mut next = current.clone();
            next.projects.retain(|project| project.id != id);
            inner.store.save(&next)?;
            *current = next;
            Ok(())
        })
        .await
    }
}
