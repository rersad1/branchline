//! Non-recursive, bounded environment-local directory discovery. Never reads file contents.
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRequest {
    pub path: String,
    pub offset: usize,
}
#[derive(Deserialize, Serialize)]
pub struct DirectoryPage {
    pub environment_id: Uuid,
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirectoryEntry>,
    pub next_offset: Option<usize>,
}
#[derive(Deserialize, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub symlink: bool,
}
#[derive(Debug, thiserror::Error)]
pub enum DirectoryError {
    #[error("An absolute Linux directory path is required")]
    InvalidPath,
    #[error("Directory is unavailable or permission was denied")]
    Unavailable,
    #[error("Directory is too large to browse; enter the Linux path manually")]
    Limit,
}
pub async fn browse(id: Uuid, request: DirectoryRequest) -> Result<DirectoryPage, DirectoryError> {
    if request.path.len() > 4096
        || !crate::environment::valid_path(&request.path)
        || request.offset > 10000
    {
        return Err(DirectoryError::InvalidPath);
    }
    tokio::task::spawn_blocking(move || {
        let path = crate::environment::canonical_path(Path::new(&request.path))
            .map_err(|_| DirectoryError::Unavailable)?;
        let text = path.to_str().ok_or(DirectoryError::InvalidPath)?.to_owned();
        let mut entries = Vec::new();
        let reader = std::fs::read_dir(&path).map_err(|_| DirectoryError::Unavailable)?;
        for (count, item) in reader.enumerate() {
            if count >= 10000 {
                return Err(DirectoryError::Limit);
            }
            let item = item.map_err(|_| DirectoryError::Unavailable)?;
            let kind = item.file_type().map_err(|_| DirectoryError::Unavailable)?;
            // Follow directory symlinks only for navigation; each subsequent browse/add
            // canonicalizes again. Broken/inaccessible symlinks are omitted.
            if !kind.is_dir() && !(kind.is_symlink() && item.path().is_dir()) {
                continue;
            }
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(child) = item.path().to_str().map(str::to_owned) else {
                continue;
            };
            if child.len() > 4096 {
                continue;
            }
            entries.push(DirectoryEntry {
                name,
                path: child,
                symlink: kind.is_symlink(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let next_offset = (entries.len() > request.offset + 50).then_some(request.offset + 50);
        Ok(DirectoryPage {
            environment_id: id,
            path: text,
            parent: path.parent().and_then(|p| p.to_str()).map(str::to_owned),
            entries: entries.into_iter().skip(request.offset).take(50).collect(),
            next_offset,
        })
    })
    .await
    .map_err(|_| DirectoryError::Unavailable)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pagination_and_invalid_paths() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..55 {
            std::fs::create_dir(root.path().join(format!("dir-{n:02}"))).unwrap();
        }
        std::fs::write(root.path().join("file"), "not returned").unwrap();
        let id = Uuid::new_v4();
        let first = browse(
            id,
            DirectoryRequest {
                path: root.path().to_str().unwrap().into(),
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(first.environment_id, id);
        assert_eq!(first.entries.len(), 50);
        assert_eq!(first.next_offset, Some(50));
        let last = browse(
            id,
            DirectoryRequest {
                path: first.path,
                offset: 50,
            },
        )
        .await
        .unwrap();
        assert_eq!(last.entries.len(), 5);
        assert_eq!(last.next_offset, None);
        assert!(browse(
            id,
            DirectoryRequest {
                path: "relative".into(),
                offset: 0
            }
        )
        .await
        .is_err());
        assert!(browse(
            id,
            DirectoryRequest {
                path: root.path().join("missing").to_str().unwrap().into(),
                offset: 0
            }
        )
        .await
        .is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_and_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, root.path().join("alias")).unwrap();
        let id = Uuid::new_v4();
        let page = browse(
            id,
            DirectoryRequest {
                path: root.path().to_str().unwrap().into(),
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert!(page.entries.iter().any(|e| e.name == "alias" && e.symlink));
        let resolved = browse(
            id,
            DirectoryRequest {
                path: root.path().join("alias").to_str().unwrap().into(),
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(resolved.path, target.to_str().unwrap());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o0)).unwrap();
        let result = browse(
            id,
            DirectoryRequest {
                path: target.to_str().unwrap().into(),
                offset: 0,
            },
        )
        .await;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(result, Err(DirectoryError::Unavailable)));
    }
}
