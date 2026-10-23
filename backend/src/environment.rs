//! Environment-local service identity. Remote dispatch belongs to D1.3.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    Windows,
    Wsl,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExecutionEnvironment {
    pub id: Uuid,
    pub kind: EnvironmentKind,
    pub distro: Option<String>,
}

impl ExecutionEnvironment {
    pub fn local() -> Self {
        #[cfg(windows)]
        let (kind, distro, machine) = (
            EnvironmentKind::Windows,
            None,
            crate::git::windows::machine_identity().unwrap_or_default(),
        );
        #[cfg(unix)]
        let (kind, distro, machine) = (
            EnvironmentKind::Wsl,
            std::env::var("WSL_DISTRO_NAME").ok(),
            std::fs::read_to_string("/etc/machine-id").unwrap_or_default(),
        );
        let key = format!("{kind:?}\0{distro:?}\0{machine}");
        Self {
            id: Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()),
            kind,
            distro,
        }
    }

    /// A service executes only in its own host; it never changes Git based on a path.
    pub fn verify_local(&self) -> Result<(), &'static str> {
        #[cfg(unix)]
        if self.distro.as_ref().is_none_or(|d| d.trim().is_empty())
            || std::fs::read_to_string("/etc/machine-id").map_or(true, |id| id.trim().is_empty())
        {
            return Err("An explicit WSL distro and machine identity are required");
        }
        #[cfg(windows)]
        if crate::git::windows::machine_identity().is_none() {
            return Err("Windows installation identity is unavailable");
        }
        if self != &Self::local() {
            return Err("Environment does not match this service host");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectDto {
    pub environment_id: Uuid,
    #[serde(flatten)]
    pub project: crate::config::Project,
}

pub fn valid_path(value: &str) -> bool {
    if value.contains('\0') {
        return false;
    }
    #[cfg(unix)]
    {
        value.starts_with('/') && !value.starts_with("//")
    }
    #[cfg(windows)]
    {
        let value = value.strip_prefix(r"\\?\").unwrap_or(value);
        let b = value.as_bytes();
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\')
    }
}

/// Normalize the Win32 verbatim prefix so missing Git records can match last-known paths.
pub fn canonical_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let path = path.canonicalize()?;
    #[cfg(windows)]
    if let Some(text) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        if !valid_path(text) {
            return Err(std::io::Error::other("Unsupported filesystem location"));
        }
        return Ok(text.into());
    }
    Ok(path)
}

#[cfg(windows)]
pub fn supported_git_directory(path: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    // Git for Windows 2.46 cannot chdir to a repo root beyond MAX_PATH.
    // Refuse explicitly instead of producing a misleading availability/clean result.
    path.as_os_str().encode_wide().count() < 260
}
