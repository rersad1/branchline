//! Environment-local installed executables, independent from processes and sessions.
use crate::{environment::canonical_path, service::WorkspaceService};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;
pub mod npm;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Acp,
    Native,
    Terminal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub id: Uuid,
    pub environment_id: Uuid,
    pub name: String,
    pub executable: PathBuf,
    pub version: Option<String>,
    pub protocol: Protocol,
    pub compatibility: String,
    pub capabilities: Vec<String>,
    pub manual: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm: Option<npm::Launch>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Discover,
    Add {
        executable: String,
        #[serde(default)]
        node: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub request_id: Uuid,
    #[serde(flatten)]
    pub action: Action,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub request_id: Uuid,
    pub installations: Vec<Installation>,
}
#[derive(Debug, thiserror::Error)]
#[error("{}", message(.0))]
pub struct Error(pub &'static str);
pub fn message(code: &str) -> &'static str {
    match code {
        "harness_scope" => "Harness/environment/worktree identity is not confirmed.",
        "harness_path" => "Executable is unavailable, not absolute, or requires a shell wrapper. Select the actual executable.",
        "harness_package" => "Select a pinned D7 npm package directory or its published entrypoint. Package/bin/dependency pins must match.",
        "harness_node" => "Select an absolute environment-local Node executable with a supported version.",
        "harness_limit" => "Harness queue, frame, instance or session limit exceeded.",
        "harness_incompatible" => "Harness version/protocol is unverified or incompatible. See the capability matrix.",
        "harness_protocol" => "Harness returned an invalid ACP message.",
        "harness_timeout" => "Harness did not respond before the deadline. Connection is stale; stop and explicitly relaunch.",
        "harness_permission" => "Permission is no longer pending or the selected option is invalid.",
        "harness_busy" => "Harness is busy; cancel the active turn or wait for its completion.",
        _ => "Harness disconnected or could not start. Credentials remain in the harness environment.",
    }
}
#[derive(Clone, Default)]
pub struct Registry {
    entries: Arc<Mutex<BTreeMap<Uuid, Installation>>>,
    gate: Arc<tokio::sync::Mutex<()>>,
}
pub const NAMES: &[&str] = &[
    "opencode",
    "gemini",
    "qwen",
    "codex",
    "codex-acp",
    "claude",
    "claude-agent-acp",
];
/// Declarative launch policy; standard ACP candidates use the same client.
pub struct Descriptor {
    pub name: &'static str,
    pub argv: &'static [&'static str],
    pub verified_version: Option<&'static str>,
}
pub fn descriptor(name: &str) -> Option<Descriptor> {
    let (name, argv, version): (&'static str, &'static [&'static str], Option<&'static str>) =
        match name {
            "opencode" => (
                "opencode",
                &[
                    "acp",
                    "--hostname",
                    "127.0.0.1",
                    "--port",
                    "0",
                    "--mdns=false",
                ],
                Some("1.19.0"),
            ),
            "gemini" => ("gemini", &["--acp"], None),
            "qwen" => ("qwen", &["--acp"], None),
            "codex-acp" => ("codex-acp", &[], None),
            "claude-agent-acp" => ("claude-agent-acp", &[], None),
            _ => return None,
        };
    Some(Descriptor {
        name,
        argv,
        verified_version: version,
    })
}
pub fn executable(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() || !path.is_file() {
        return Err(Error("harness_path"));
    }
    let path = canonical_path(path).map_err(|_| Error("harness_path"))?;
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    {
        return Err(Error("harness_path"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&path)
            .map_err(|_| Error("harness_path"))?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(Error("harness_path"));
        }
    }
    Ok(path)
}
impl Registry {
    pub fn get(&self, id: Uuid) -> Result<Installation, Error> {
        self.entries
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(Error("harness_scope"))
    }
    pub async fn execute(&self, service: &WorkspaceService, r: Request) -> Result<Response, Error> {
        let _gate = self.gate.try_lock().map_err(|_| Error("harness_busy"))?;
        if r.request_id.is_nil() {
            return Err(Error("harness_scope"));
        }
        let manual = matches!(r.action, Action::Add { .. });
        let (candidates, node) = match r.action {
            Action::Add {
                executable: p,
                node,
            } => (vec![PathBuf::from(p)], node),
            Action::Discover => (candidates(), None),
        };
        // No agent launch/handshake on discovery. Only known fixed --version probes.
        let started = std::time::Instant::now();
        for path in candidates.into_iter().take(64) {
            let npm_path = path.is_dir()
                || path
                    .extension()
                    .is_some_and(|e| e == "cmd" || e == "js" || e == "mjs")
                || canonical_path(&path)
                    .ok()
                    .is_some_and(|p| p.extension().is_some_and(|e| e == "js" || e == "mjs"));
            let npm = if npm_path || node.is_some() {
                match npm::resolve(&path, node.as_deref().map(Path::new)) {
                    Ok(npm) => Some(npm),
                    Err(e) if manual => return Err(e),
                    Err(_) => continue,
                }
            } else {
                None
            };
            let path = if let Some(npm) = &npm {
                npm.entry.clone()
            } else {
                match executable(&path) {
                    Ok(p) => p,
                    Err(e) if manual => return Err(e),
                    Err(_) => continue,
                }
            };
            let key = format!(
                "{}\0{}",
                path.display(),
                npm.as_ref()
                    .map(|n| n.node.to_string_lossy())
                    .unwrap_or_default()
            );
            let id = Uuid::new_v5(&service.environment.id, key.as_bytes());
            if self.entries.lock().unwrap().contains_key(&id) {
                continue;
            }
            if self.entries.lock().unwrap().len() >= 64 {
                return Err(Error("harness_limit"));
            }
            let stem = npm.as_ref().map(|n| n.name.as_str()).unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("manual")
            });
            let known = NAMES.contains(&stem);
            let version = if known && started.elapsed() < std::time::Duration::from_secs(8) {
                if let Some(npm) = &npm {
                    crate::acp::npm_version(npm).await.ok()
                } else {
                    crate::acp::version(&path).await.ok()
                }
            } else {
                None
            };
            let verified = descriptor(stem).is_some_and(|d| {
                d.verified_version.is_some() && d.verified_version == version.as_deref()
            });
            let installation = Installation {
                id,
                environment_id: service.environment.id,
                name: stem.into(),
                executable: path.clone(),
                version: version.clone(),
                protocol: if verified {
                    Protocol::Acp
                } else {
                    Protocol::Terminal
                },
                compatibility: if verified {
                    "pinned; handshake required"
                } else if npm.is_some() && version.is_some() {
                    "D7 candidate; acceptance incomplete; explicit ACP test only"
                } else {
                    "unverified; terminal only"
                }
                .into(),
                capabilities: if verified {
                    vec![
                        "ACP v1 negotiation".into(),
                        "OpenCode HTTP/SSE diagnostics".into(),
                    ]
                } else {
                    if npm.is_some() && version.is_some() {
                        vec!["terminal".into(), "D7 candidate validation".into()]
                    } else {
                        vec!["terminal".into()]
                    }
                },
                manual,
                npm,
            };
            self.entries.lock().unwrap().insert(id, installation);
        }
        Ok(Response {
            environment_id: service.environment.id,
            request_id: r.request_id,
            installations: self.entries.lock().unwrap().values().cloned().collect(),
        })
    }
}
fn candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|s| {
            std::env::split_paths(&s)
                .filter(|p| p.is_absolute())
                .take(96)
                .collect()
        })
        .unwrap_or_default();
    for home in [std::env::var_os("HOME"), std::env::var_os("USERPROFILE")]
        .into_iter()
        .flatten()
    {
        for suffix in [
            ".opencode/bin",
            ".local/bin",
            ".npm-global/bin",
            ".cargo/bin",
        ] {
            dirs.push(PathBuf::from(&home).join(suffix));
        }
    }
    let mut paths = Vec::new();
    for dir in dirs {
        for name in NAMES {
            let p = dir.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            });
            if p.is_file() {
                paths.push(p);
            }
            #[cfg(windows)]
            if dir.join(format!("{name}.cmd")).is_file() {
                paths.push(dir.join(format!("{name}.cmd")));
            }
        }
    }
    paths
}

/// Reuse canonical root/common-dir/admin identity discovery; never trust UI cwd.
pub async fn directory(
    service: &WorkspaceService,
    project: Uuid,
    tree: Uuid,
) -> Result<PathBuf, Error> {
    let snapshot = service
        .worktrees
        .refresh(project)
        .await
        .map_err(|_| Error("harness_scope"))?;
    let t = snapshot
        .worktrees
        .iter()
        .find(|t| t.id == tree)
        .ok_or(Error("harness_scope"))?;
    if snapshot.environment_id != service.environment.id
        || snapshot.error.is_some()
        || !t.is_available
        || t.is_bare
        || t.availability_error.is_some()
    {
        return Err(Error("harness_scope"));
    }
    canonical_path(&t.path).map_err(|_| Error("harness_scope"))
}
