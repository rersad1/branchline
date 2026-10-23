use super::{Error, LIMIT};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
use uuid::Uuid;

pub struct Opened {
    file: File,
    _parents: Vec<File>,
    identity: Vec<u8>,
    #[cfg(unix)]
    expected: std::path::PathBuf,
}
impl Opened {
    pub fn read(&mut self) -> Result<Vec<u8>, Error> {
        self.validate_location()?;
        if self.file.metadata()?.len() > LIMIT as u64 {
            return Err(Error("edit_large"));
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        (&mut self.file)
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > LIMIT {
            return Err(Error("edit_large"));
        }
        self.validate_location()?;
        Ok(bytes)
    }
    pub fn version(&self, bytes: &[u8]) -> String {
        let identity = Uuid::new_v5(&Uuid::NAMESPACE_OID, &self.identity);
        Uuid::new_v5(&identity, bytes).to_string()
    }
    pub fn write(&mut self, bytes: &[u8], before: &[u8]) -> Result<(), Error> {
        self.validate_location()?;
        if self.file.metadata()?.permissions().readonly() {
            return Err(Error("edit_io"));
        }
        if self.read()? != before {
            return Err(Error("edit_conflict"));
        }
        self.file.seek(SeekFrom::Start(0))?;
        // Hold the validated file handle for the entire operation. Never reopen a pathname.
        let result = self
            .file
            .write_all(bytes)
            .and_then(|_| self.file.set_len(bytes.len() as u64))
            .and_then(|_| self.file.sync_all());
        if result.is_err() {
            // Best-effort rollback on an I/O failure; keep the UI draft regardless.
            let _ = self.file.seek(SeekFrom::Start(0));
            let _ = self
                .file
                .write_all(before)
                .and_then(|_| self.file.set_len(before.len() as u64))
                .and_then(|_| self.file.sync_all());
            return Err(Error("edit_io"));
        }
        self.validate_location()?;
        if self.read()? != bytes {
            return Err(Error("edit_conflict"));
        }
        Ok(())
    }
    fn validate_location(&self) -> Result<(), Error> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
            let current =
                std::fs::symlink_metadata(&self.expected).map_err(|_| Error("edit_conflict"))?;
            let held = self.file.metadata()?;
            let actual = std::fs::read_link(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
                .map_err(|_| Error("edit_path"))?;
            if actual != self.expected
                || current.dev() != held.dev()
                || current.ino() != held.ino()
                || held.nlink() != 1
            {
                return Err(Error("edit_conflict"));
            }
        }
        Ok(())
    }
}
#[cfg(unix)]
pub fn open(root: &Path, relative: &str, write: bool) -> Result<Opened, Error> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let root_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")?;
    let mut parents = vec![root_file];
    let root_parts = root
        .components()
        .filter_map(|p| {
            if let std::path::Component::Normal(p) = p {
                p.to_str()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let root_index = root_parts.len();
    let components = root_parts
        .into_iter()
        .chain(relative.split('/'))
        .collect::<Vec<_>>();
    let parts = components;
    for (i, part) in parts.iter().enumerate() {
        let leaf = i + 1 == parts.len();
        let name = std::ffi::CString::new(*part).map_err(|_| Error("edit_path"))?;
        let flags = libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if leaf {
                if write {
                    libc::O_RDWR
                } else {
                    libc::O_RDONLY
                }
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY
            };
        // SAFETY: live anchored directory, single NUL-terminated component, owned returned fd.
        let fd = unsafe { libc::openat(parents.last().unwrap().as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(Error(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied {
                    "edit_io"
                } else {
                    "edit_path"
                },
            ));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if leaf {
            let meta = file.metadata()?;
            if !meta.is_file() || meta.nlink() != 1 {
                return Err(Error("edit_path"));
            }
            let root_meta = parents[root_index].metadata()?;
            let identity = format!(
                "{}:{}:{}:{}",
                root_meta.dev(),
                root_meta.ino(),
                meta.dev(),
                meta.ino()
            )
            .into_bytes();
            return Ok(Opened {
                file,
                _parents: parents,
                identity,
                expected: root.join(relative),
            });
        }
        parents.push(file);
    }
    Err(Error("edit_path"))
}
#[cfg(windows)]
pub fn open(root: &Path, relative: &str, write: bool) -> Result<Opened, Error> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut parents = vec![];
    let mut path = std::path::PathBuf::new();
    // Pin every ancestor, including ancestors of the registered worktree, against rename.
    for component in root.components() {
        path.push(component);
        if !path.has_root() {
            continue;
        }
        let dir = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&path)?;
        if dir.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Error("edit_path"));
        }
        parents.push(dir);
    }
    let root_index = parents.len().checked_sub(1).ok_or(Error("edit_path"))?;
    let parts: Vec<_> = relative.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        path.push(part);
        let leaf = i + 1 == parts.len();
        let file = OpenOptions::new()
            .read(true)
            .write(write && leaf)
            .share_mode(if leaf {
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            })
            .custom_flags(
                FILE_FLAG_OPEN_REPARSE_POINT | if leaf { 0 } else { FILE_FLAG_BACKUP_SEMANTICS },
            )
            .open(&path)?;
        let meta = file.metadata()?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Error("edit_path"));
        }
        if leaf {
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            let mut root_info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            let root_handle = &parents[root_index];
            // SAFETY: live handle and writable initialized result struct.
            if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0
                || unsafe {
                    GetFileInformationByHandle(root_handle.as_raw_handle() as _, &mut root_info)
                } == 0
                || !meta.is_file()
                || info.nNumberOfLinks != 1
            {
                return Err(Error("edit_path"));
            }
            let identity = format!(
                "{}:{}:{}:{}:{}:{:?}",
                info.dwVolumeSerialNumber,
                root_info.nFileIndexHigh,
                root_info.nFileIndexLow,
                info.nFileIndexHigh,
                info.nFileIndexLow,
                root
            )
            .into_bytes();
            return Ok(Opened {
                file,
                _parents: parents,
                identity,
            });
        }
        parents.push(file);
    }
    Err(Error("edit_path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn nested_revision_uses_worktree_root_not_immediate_parent() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/file"), "same").unwrap();
        let root = crate::environment::canonical_path(&root).unwrap();
        let before = {
            let mut held = open(&root, "nested/file", false).unwrap();
            let bytes = held.read().unwrap();
            held.version(&bytes)
        };
        let moved = fixture.path().join("moved");
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::rename(moved.join("nested"), root.join("nested")).unwrap();
        let mut held = open(&root, "nested/file", false).unwrap();
        let bytes = held.read().unwrap();
        assert_ne!(held.version(&bytes), before);
    }
    #[cfg(unix)]
    #[test]
    fn replaced_open_file_never_acknowledges_save() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), "before").unwrap();
        let mut held = open(dir.path(), "file", true).unwrap();
        let before = held.read().unwrap();
        std::fs::write(dir.path().join("replacement"), "external").unwrap();
        std::fs::rename(dir.path().join("replacement"), dir.path().join("file")).unwrap();
        assert_eq!(held.write(b"mine", &before).unwrap_err().0, "edit_conflict");
        assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), b"external");
    }
}
