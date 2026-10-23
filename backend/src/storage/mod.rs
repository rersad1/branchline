use crate::config::Config;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Configuration I/O failed")]
    Io(#[from] io::Error),
    #[error("Invalid configuration JSON")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(&'static str),
}

#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Result<Self, StorageError> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(StorageError::Invalid(
                "Configuration path must be an absolute file path",
            ));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Config, StorageError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A dangling symlink is an error, not an absent configuration.
                if fs::symlink_metadata(&self.path).is_ok() {
                    return Err(StorageError::Io(error));
                }
                return Ok(Config::default());
            }
            Err(error) => return Err(error.into()),
        };
        let config: Config = serde_json::from_slice(&bytes)?;
        config.validate().map_err(StorageError::Invalid)?;
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<(), StorageError> {
        self.save_before_commit(config, |_| Ok(()))
    }

    /// Retain the validated v1 bytes before the desktop first uses this registry.
    /// Called with the registry's exclusive lock held. Never replace an earlier backup.
    pub fn backup_v1(&self) -> Result<(), StorageError> {
        self.load()?;
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut backup = self.path.as_os_str().to_owned();
        backup.push(".v1.bak");
        let backup = PathBuf::from(backup);
        if fs::symlink_metadata(&backup).is_ok() {
            return Ok(());
        }
        let mut temporary = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(backup)
            .map_err(|e| StorageError::Io(e.error))?;
        Ok(())
    }

    fn save_before_commit(
        &self,
        config: &Config,
        before_commit: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> Result<(), StorageError> {
        config.validate().map_err(StorageError::Invalid)?;
        let bytes = serde_json::to_vec_pretty(config)?;
        let parent = self
            .path
            .parent()
            .ok_or(StorageError::Invalid("Missing configuration directory"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        // tempfile creates mode 0600 on Linux. Write and fsync before replacing.
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        before_commit(temporary.as_file_mut())?;
        temporary
            .persist(&self.path)
            .map_err(|error| StorageError::Io(error.error))?;
        // Rename is the commit point. Do not report a rollback after it succeeded.
        if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
            tracing::warn!(kind = ?error.kind(), "configuration committed; directory sync failed");
        }
        Ok(())
    }
}

/// Same-directory, fsynced atomic replacement, shared by desktop reference settings.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let parent = path
        .parent()
        .ok_or(StorageError::Invalid("Missing directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|e| StorageError::Io(e.error))?;
    #[cfg(unix)]
    let _ = File::open(parent).and_then(|directory| directory.sync_all());
    Ok(())
}

pub fn default_config_path() -> Result<PathBuf, StorageError> {
    if let Some(path) = std::env::var_os("AWM_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(StorageError::Invalid("LOCALAPPDATA is required"))?;
    #[cfg(unix)]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or(StorageError::Invalid("HOME or XDG_CONFIG_HOME is required"))?;
    Ok(base.join("agent-workspace/config.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Project;
    use chrono::Utc;
    use uuid::Uuid;

    fn fixture() -> Config {
        Config {
            projects: vec![Project {
                id: Uuid::new_v4(),
                name: "Проект с пробелом".into(),
                root_path: std::env::temp_dir().join("Проект с пробелом"),
                git_common_dir: std::env::temp_dir().join("Проект с пробелом/.git"),
                base_ref: Some("main".into()),
                created_at: Utc::now(),
            }],
            ..Config::default()
        }
    }

    #[test]
    fn missing_and_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("nested/config.json")).unwrap();
        assert_eq!(store.load().unwrap(), Config::default());
        let config = fixture();
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn backup_requires_valid_v1_and_never_replaces_original_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json")).unwrap();
        let backup = directory.path().join("config.json.v1.bak");
        for bytes in ["broken", r#"{"version":99,"projects":[]}"#] {
            fs::write(store.path(), bytes).unwrap();
            assert!(store.backup_v1().is_err());
            assert!(!backup.exists());
            assert_eq!(fs::read_to_string(store.path()).unwrap(), bytes);
        }
        store.save(&fixture()).unwrap();
        let original = fs::read(store.path()).unwrap();
        store.backup_v1().unwrap();
        store.save(&Config::default()).unwrap();
        store.backup_v1().unwrap();
        assert_eq!(fs::read(backup).unwrap(), original);
    }

    #[test]
    fn failed_write_keeps_old_bytes_and_cleans_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json")).unwrap();
        let old = fixture();
        store.save(&old).unwrap();
        let bytes = fs::read(store.path()).unwrap();
        let result = store.save_before_commit(&Config::default(), |_| {
            Err(io::Error::other("injected pre-rename failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
        assert_eq!(store.load().unwrap(), old);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let mut invalid = old;
        invalid.version = 99;
        assert!(store.save(&invalid).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), bytes);
    }

    #[test]
    fn corrupt_unsupported_and_secret_fields_are_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json")).unwrap();
        for bytes in [
            "broken",
            r#"{"version":99,"projects":[]}"#,
            r#"{"version":1,"projects":[],"opencode_api_key":"secret"}"#,
        ] {
            fs::write(store.path(), bytes).unwrap();
            assert!(store.load().is_err());
            assert_eq!(fs::read_to_string(store.path()).unwrap(), bytes);
        }
        let mut duplicate = fixture();
        duplicate.projects.push(duplicate.projects[0].clone());
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn failed_rename_leaves_destination_and_removes_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("sentinel"), "untouched").unwrap();
        let store = ConfigStore::new(path.clone()).unwrap();
        assert!(store.save(&Config::default()).is_err());
        assert_eq!(
            fs::read_to_string(path.join("sentinel")).unwrap(),
            "untouched"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn permission_failure_preserves_existing_file() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("config.json")).unwrap();
        store.save(&fixture()).unwrap();
        let before = fs::read(store.path()).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let result = store.save(&Config::default());
        // Restore before assertions so fixture cleanup also works after a failure.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        if std::env::var("USER").as_deref() == Ok("root") {
            eprintln!("permission check requires an unprivileged Linux user");
            return;
        }
        assert!(result.is_err());
        assert_eq!(fs::read(store.path()).unwrap(), before);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
