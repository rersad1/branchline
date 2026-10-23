//! Windows service plus explicitly selected, privately bootstrapped WSL workers.
use agent_workspace_backend::{
    environment::{EnvironmentKind, ExecutionEnvironment, ProjectDto},
    projects::Registry,
    service::WorkspaceService,
    storage::default_config_path,
    worker::{Handshake, Ready, Startup, PROTOCOL, VERSION},
    worktrees::Snapshot,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tauri::Emitter;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::Mutex,
};
use uuid::Uuid;

const ASSET: &[u8] = include_bytes!("../assets/worker-linux-x86_64");
const BOOTSTRAP: &str = concat!(
    include_str!("helper_assets.py"),
    "\n",
    include_str!("bootstrap.py")
);
#[derive(Deserialize)]
#[serde(untagged)]
enum BootstrapReply {
    Ready(Ready),
    Failed { error: String },
}
fn bootstrap_reply(bytes: &[u8]) -> Result<Ready, Problem> {
    match serde_json::from_slice::<BootstrapReply>(bytes) {
        Ok(BootstrapReply::Ready(ready)) if ready.port != 0 => Ok(ready),
        Ok(BootstrapReply::Failed { error }) => Err(Problem::new(match error.as_str() {
            "helper_platform" => "helper_platform",
            "helper_prerequisite" => "helper_prerequisite",
            "helper_temp" => "helper_temp",
            "helper_storage" => "helper_storage",
            "helper_asset" => "helper_asset",
            _ => "startup_failed",
        })),
        _ => Err(Problem::new("startup_failed")),
    }
}
async fn exchange_bootstrap<W: AsyncWrite + Unpin, R: AsyncRead + Unpin>(
    stdin: &mut W,
    stdout: &mut R,
    startup: &Startup,
) -> Result<Ready, Problem> {
    let send = async {
        let header = serde_json::json!({"size":ASSET.len(), "sha256":format!("{:x}", Sha256::digest(ASSET))});
        stdin
            .write_all(format!("{header}\n").as_bytes())
            .await
            .map_err(|_| Problem::new("startup_failed"))?;
        stdin
            .write_all(ASSET)
            .await
            .map_err(|_| Problem::new("startup_failed"))?;
        stdin
            .write_all(&serde_json::to_vec(startup).map_err(|_| Problem::new("startup_failed"))?)
            .await
            .map_err(|_| Problem::new("startup_failed"))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|_| Problem::new("startup_failed"))
    };
    let receive = async {
        bootstrap_reply(
            &agent_workspace_backend::worker::read_line(stdout, 128)
                .await
                .map_err(|_| Problem::new("startup_failed"))?,
        )
    };
    // A prerequisite refusal can arrive before Python consumes the asset. Preserve
    // its fixed diagnostic even when the simultaneous write gets BrokenPipe.
    let (sent, received) = tokio::join!(send, receive);
    let ready = received?;
    sent?;
    Ok(ready)
}
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
fn domain_problem(code: &str) -> Problem {
    let message = match code {
        c if c.starts_with("harness_") => agent_workspace_backend::harness::message(c),
        c if c.starts_with("provider_") => agent_workspace_backend::provider::message(c),
        c if c.starts_with("remove_") => agent_workspace_backend::remove_worktree::message(c),
        c if c.starts_with("terminal_") => agent_workspace_backend::terminal::message(c),
        c if c.starts_with("create_") => agent_workspace_backend::create_worktree::message(c),
        c if c.starts_with("edit_") => agent_workspace_backend::edit::message(c),
        "invalid_diff" => "Invalid scoped diff request",
        "worktree_unavailable" => "Worktree is unavailable or its identity changed",
        "diff_file_not_found" => "Selected file is no longer in this scoped diff",
        "diff_limit" => "Git diff output limit exceeded",
        "diff_failed" => "Git diff could not be read",
        "diff_changed" => "Git diff changed while reading; refresh required",
        "duplicate_project" => "Этот Git repository уже добавлен, включая его linked worktree.",
        "not_repository" => "Выбранная папка не является Git repository.",
        "project_unavailable" | "directory_unavailable" => {
            "Папка недоступна. Проверьте путь и права доступа выбранной среды."
        }
        "invalid_path" | "validation_error" => {
            "Нужен абсолютный путь в выбранной среде. Network/UNC paths не поддерживаются. Windows root должен быть короче 260 UTF-16 символов."
        }
        "directory_limit" => "Каталог слишком большой для просмотра. Введите Linux path вручную.",
        "git_unavailable" => "Git недоступен в выбранной среде. Установите Git и проверьте PATH.",
        "git_timeout" => "Проверка Git превысила время ожидания.",
        "not_found" => "Запись не найдена.",
        _ => return Problem::new("operation_failed"),
    };
    Problem {
        code: code.into(),
        message: message.into(),
    }
}
fn registry_problem(error: agent_workspace_backend::projects::RegistryError) -> Problem {
    use agent_workspace_backend::projects::{ProjectError, RegistryError};
    domain_problem(match error {
        RegistryError::Project(ProjectError::Duplicate) => "duplicate_project",
        RegistryError::Project(ProjectError::NotRepository) => "not_repository",
        RegistryError::Project(ProjectError::Unavailable) => "project_unavailable",
        RegistryError::Project(ProjectError::InvalidPath | ProjectError::UnsupportedPath) => {
            "invalid_path"
        }
        RegistryError::Project(ProjectError::GitUnavailable) => "git_unavailable",
        RegistryError::Project(ProjectError::GitTimeout) => "git_timeout",
        RegistryError::ActiveTerminal => "terminal_active",
        RegistryError::InvalidInput(_) => "validation_error",
        _ => "operation_failed",
    })
}
#[derive(Clone, Debug, Serialize)]
pub struct Problem {
    pub code: String,
    pub message: String,
}
impl Problem {
    pub(crate) fn new(code: &str) -> Self {
        Self {
            code: code.into(),
            message: match code {
                "incompatible_worker" => "Worker version is incompatible",
                "identity_mismatch" => {
                    "Worker environment identity changed; explicit relink required"
                }
                "unknown_environment" => "Environment is not connected",
                "startup_failed" => "WSL worker could not start. Check that WSL2 and the named distro are installed, with Python3 at /usr/bin/python3, Linux Git and an executable temporary directory. Windows remains available.",
                "helper_platform" => "Linux helper requires Linux x86_64, GNU glibc >=2.39 and /lib64/ld-linux-x86-64.so.2. The distro name and release number are not restricted. Windows remains available.",
                "helper_prerequisite" => "Linux helper requires Python >=3.10 at /usr/bin/python3, Git at /usr/bin/git and Bash at /bin/bash in the selected WSL distro.",
                "helper_temp" => "Linux helper cannot execute in the selected temporary directory. Use an executable private temp location in Ubuntu; Windows remains available.",
                "helper_storage" => "Linux helper cannot open its private state directory. Check XDG_STATE_HOME or ~/.local/state/branchline permissions; symlink state paths are refused.",
                "helper_asset" => "Linux helper asset is incomplete or failed integrity validation. Reinstall this Branchline build.",
                "local_unavailable" => {
                    "Windows workspace service could not initialize its registry"
                }
                "disconnected" => "Worker disconnected; retry connection",
                "invalid_request" => "Invalid request",
                _ => "Workspace operation failed",
            }
            .into(),
        }
    }
}
#[derive(Clone, Serialize)]
pub struct RuntimeState {
    pub environment: ExecutionEnvironment,
    pub state: String,
    pub error: Option<Problem>,
}
struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    #[cfg(windows)]
    _job: agent_workspace_backend::git::windows::Job,
    peer: Peer,
}
#[derive(Clone)]
struct Peer {
    port: u16,
    token: String,
    nonce: String,
    environment: ExecutionEnvironment,
}
impl std::ops::Deref for Worker {
    type Target = Peer;
    fn deref(&self) -> &Peer {
        &self.peer
    }
}
impl Peer {
    async fn send_empty(&self, request: reqwest::RequestBuilder) -> Result<(), Problem> {
        let mut response = request
            .send()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(domain_problem("not_found"))
        } else {
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| Problem::new("disconnected"))?
            {
                if bytes.len() + chunk.len() > MAX_RESPONSE {
                    return Err(Problem::new("response_limit"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| Problem::new("operation_failed"))?;
            Err(domain_problem(
                value["error"]["code"]
                    .as_str()
                    .unwrap_or("operation_failed"),
            ))
        }
    }
    async fn request<T: serde::de::DeserializeOwned>(
        &self,
        client: &reqwest::Client,
        path: &str,
    ) -> Result<T, Problem> {
        self.send(
            client
                .get(format!("http://127.0.0.1:{}/api/v1/{path}", self.port))
                .bearer_auth(&self.token),
        )
        .await
    }
    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        client: &reqwest::Client,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, Problem> {
        self.send(
            client
                .post(format!("http://127.0.0.1:{}/api/v1/{path}", self.port))
                .bearer_auth(&self.token)
                .json(&body),
        )
        .await
    }
    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, Problem> {
        let mut response = request
            .send()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        let success = response.status().is_success();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Problem::new("disconnected"))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(Problem::new("response_limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !success {
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| Problem::new("operation_failed"))?;
            // Only known sanitized domain errors cross IPC; never forward arbitrary peer text.
            let code = value["error"]["code"]
                .as_str()
                .unwrap_or("operation_failed");
            return Err(domain_problem(code));
        }
        serde_json::from_slice(&bytes).map_err(|_| Problem::new("invalid_response"))
    }
    async fn verify(&self, client: &reqwest::Client) -> Result<(), Problem> {
        let h: Handshake = self.request(client, "runtime/handshake").await?;
        validate_handshake(&h, &self.nonce, &self.environment)
    }
}
fn registration_id(distro: &str) -> Result<String, Problem> {
    #[cfg(windows)]
    {
        agent_workspace_backend::git::windows::distro_registration(distro)
            .ok_or_else(|| Problem::new("startup_failed"))
    }
    #[cfg(not(windows))]
    {
        let _ = distro;
        Err(Problem::new("startup_failed"))
    }
}
impl Worker {
    async fn stop(mut self) {
        self.stdin.take();
        if tokio::time::timeout(Duration::from_secs(12), self.child.wait())
            .await
            .is_err()
        {
            // Only our wsl launcher. Linux lease expires independently, never distro shutdown.
            let _ = self.child.kill().await;
        }
    }
}
fn validate_handshake(
    h: &Handshake,
    nonce: &str,
    expected: &ExecutionEnvironment,
) -> Result<(), Problem> {
    if h.protocol != PROTOCOL || h.version != VERSION {
        return Err(Problem::new("incompatible_worker"));
    }
    if h.nonce != nonce || &h.environment != expected {
        return Err(Problem::new("identity_mismatch"));
    }
    Ok(())
}
pub struct Runtime {
    workspace: Mutex<Result<crate::workspace::WorkspaceStore, Problem>>,
    local: Result<WorkspaceService, Problem>,
    workers: Mutex<HashMap<Uuid, Worker>>,
    states: Mutex<HashMap<Uuid, RuntimeState>>,
    restarted: Mutex<HashSet<Uuid>>,
    monitor: Mutex<()>,
    stopping: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    // Serializes connect/launch separately from service reads; bounds worker count.
    launch: Mutex<()>,
    requests: tokio::sync::Semaphore,
    client: reqwest::Client,
    closing: std::sync::atomic::AtomicBool,
}
impl Runtime {
    pub async fn harnesses(
        &self,
        id: Uuid,
        request: agent_workspace_backend::harness::Request,
    ) -> Result<agent_workspace_backend::harness::Response, Problem> {
        let _permit = self
            .requests
            .try_acquire()
            .map_err(|_| domain_problem("harness_busy"))?;
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Problem::new("disconnected"));
        }
        let result: agent_workspace_backend::harness::Response =
            if self.local_environment().is_ok_and(|e| e.id == id) {
                let s = self.local.as_ref().map_err(Clone::clone)?;
                s.harnesses
                    .execute(s, request.clone())
                    .await
                    .map_err(|e| domain_problem(e.0))?
            } else {
                self.peer(id)
                    .await?
                    .post(
                        &self.client,
                        &format!("harnesses?environment_id={id}"),
                        serde_json::to_value(&request)
                            .map_err(|_| Problem::new("invalid_request"))?,
                    )
                    .await?
            };
        if result.environment_id != id
            || result.request_id != request.request_id
            || result.installations.iter().any(|i| i.environment_id != id)
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn agents(
        &self,
        id: Uuid,
        request: agent_workspace_backend::acp::Request,
    ) -> Result<agent_workspace_backend::acp::Response, Problem> {
        let bypass = matches!(
            request.action,
            agent_workspace_backend::acp::Action::Stop { .. }
                | agent_workspace_backend::acp::Action::Cancel { .. }
                | agent_workspace_backend::acp::Action::Permission { .. }
        );
        let _permit = if bypass {
            None
        } else {
            Some(
                self.requests
                    .try_acquire()
                    .map_err(|_| domain_problem("harness_busy"))?,
            )
        };
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Problem::new("disconnected"));
        }
        let result: agent_workspace_backend::acp::Response =
            if self.local_environment().is_ok_and(|e| e.id == id) {
                let s = self.local.as_ref().map_err(Clone::clone)?;
                s.agents
                    .execute(s, request.clone())
                    .await
                    .map_err(|e| domain_problem(e.0))?
            } else {
                self.peer(id)
                    .await?
                    .post(
                        &self.client,
                        &format!("agents?environment_id={id}"),
                        serde_json::to_value(&request)
                            .map_err(|_| Problem::new("invalid_request"))?,
                    )
                    .await?
            };
        if result.environment_id != id
            || result.project_id != request.project_id
            || result.worktree_id != request.worktree_id
            || result.request_id != request.request_id
            || result.instances.iter().any(|i| {
                i.environment_id != id
                    || i.project_id != request.project_id
                    || i.worktree_id != request.worktree_id
            })
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn provider(
        &self,
        id: Uuid,
        request: agent_workspace_backend::provider::Request,
    ) -> Result<agent_workspace_backend::provider::Response, Problem> {
        tokio::time::timeout(Duration::from_secs(14), async {
            // No waiting queue; cancellation bypasses occupied inspect slots.
            let _permit = if request.cancel {
                None
            } else {
                Some(
                    self.requests
                        .try_acquire()
                        .map_err(|_| domain_problem("provider_limit"))?,
                )
            };
            if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Problem::new("disconnected"));
            }
            let result = if self.local_environment().is_ok_and(|e| e.id == id) {
                let service = self.local.as_ref().map_err(Clone::clone)?;
                service
                    .provider
                    .execute(service, request.clone())
                    .await
                    .map_err(|e| domain_problem(e.0))?
            } else {
                self.peer(id)
                    .await?
                    .post(
                        &self.client,
                        &format!("provider?environment_id={id}"),
                        serde_json::to_value(&request)
                            .map_err(|_| Problem::new("invalid_request"))?,
                    )
                    .await?
            };
            if result.environment_id != id
                || result.project_id != request.project_id
                || result.worktree_id != request.worktree_id
                || result.request_id != request.request_id
                || result.port != request.port
                || result.observation.is_some() == request.cancel
            {
                return Err(Problem::new("identity_mismatch"));
            }
            Ok(result)
        })
        .await
        .map_err(|_| domain_problem("provider_timeout"))?
    }
    pub async fn terminal(
        &self,
        id: Uuid,
        request: agent_workspace_backend::terminal::Request,
    ) -> Result<agent_workspace_backend::terminal::Response, Problem> {
        let _permit = self
            .requests
            .acquire()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Problem::new("disconnected"));
        }
        let result = if self.local_environment().is_ok_and(|e| e.id == id) {
            agent_workspace_backend::terminal::execute(
                self.local.as_ref().map_err(Clone::clone)?,
                request.clone(),
            )
            .await
            .map_err(|e| domain_problem(e.0))?
        } else {
            self.peer(id)
                .await?
                .post(
                    &self.client,
                    &format!("terminals?environment_id={id}"),
                    serde_json::to_value(&request).map_err(|_| Problem::new("invalid_request"))?,
                )
                .await?
        };
        if result.environment_id != id
            || result.project_id != request.project_id
            || result.worktree_id != request.worktree_id
            || result
                .sessions
                .iter()
                .any(|s| s.project_id != request.project_id || s.worktree_id != request.worktree_id)
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn remove_worktree(
        &self,
        id: Uuid,
        request: agent_workspace_backend::remove_worktree::Request,
    ) -> Result<agent_workspace_backend::remove_worktree::Response, Problem> {
        let _permit = self
            .requests
            .acquire()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        let result = if self.local_environment().is_ok_and(|e| e.id == id) {
            agent_workspace_backend::remove_worktree::remove(
                self.local.as_ref().map_err(Clone::clone)?,
                request.clone(),
            )
            .await
            .map_err(|e| domain_problem(e.0))?
        } else {
            self.peer(id)
                .await?
                .post(
                    &self.client,
                    &format!("worktrees/remove?environment_id={id}"),
                    serde_json::to_value(&request).map_err(|_| Problem::new("invalid_request"))?,
                )
                .await?
        };
        if result.environment_id != id
            || result.project_id != request.project_id
            || result.worktree_id != request.worktree_id
            || result.snapshot.as_ref().is_some_and(|s| {
                s.environment_id != id
                    || s.project_id != request.project_id
                    || s.worktrees
                        .iter()
                        .any(|t| t.environment_id != id || t.project_id != request.project_id)
            })
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn create_worktree(
        &self,
        id: Uuid,
        request: agent_workspace_backend::create_worktree::CreateRequest,
    ) -> Result<agent_workspace_backend::create_worktree::CreateResponse, Problem> {
        let _permit = self
            .requests
            .acquire()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        // Do not cancel a local mutation at the read-only 15s outer deadline.
        let result = if self.local_environment().is_ok_and(|e| e.id == id) {
            agent_workspace_backend::create_worktree::create(
                self.local.as_ref().map_err(Clone::clone)?,
                request.clone(),
            )
            .await
            .map_err(|e| domain_problem(e.0))?
        } else {
            self.peer(id)
                .await?
                .post(
                    &self.client,
                    &format!("worktrees/create?environment_id={id}"),
                    serde_json::to_value(&request).map_err(|_| Problem::new("invalid_request"))?,
                )
                .await?
        };
        if result.environment_id != id
            || result.project_id != request.project_id
            || result.branch != request.branch
            || result.snapshot.as_ref().is_some_and(|s| {
                s.environment_id != id
                    || s.project_id != request.project_id
                    || s.worktrees
                        .iter()
                        .any(|t| t.environment_id != id || t.project_id != request.project_id)
            })
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn edit_file(
        &self,
        id: Uuid,
        request: agent_workspace_backend::edit::Request,
    ) -> Result<agent_workspace_backend::edit::Response, Problem> {
        let _permit = self
            .requests
            .acquire()
            .await
            .map_err(|_| Problem::new("disconnected"))?;
        let result = if self.local_environment().is_ok_and(|e| e.id == id) {
            agent_workspace_backend::edit::execute(
                self.local.as_ref().map_err(Clone::clone)?,
                request.clone(),
            )
            .await
            .map_err(|e| domain_problem(e.0))?
        } else {
            self.peer(id)
                .await?
                .post(
                    &self.client,
                    &format!("edit?environment_id={id}"),
                    serde_json::to_value(&request).map_err(|_| Problem::new("invalid_request"))?,
                )
                .await?
        };
        if result.environment_id != id
            || result.project_id != request.project_id
            || result.worktree_id != request.worktree_id
            || result.path != request.path
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(result)
    }
    pub async fn diff(
        &self,
        id: Uuid,
        request: agent_workspace_backend::diff::DiffRequest,
    ) -> Result<agent_workspace_backend::diff::DiffResponse, Problem> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            let result = if self.local_environment().is_ok_and(|e| e.id == id) {
                self.local
                    .as_ref()
                    .map_err(Clone::clone)?
                    .diff(request.clone())
                    .await
                    .map_err(|e| domain_problem(e.code()))?
            } else {
                self.peer(id)
                    .await?
                    .post(
                        &self.client,
                        &format!("diff?environment_id={id}"),
                        serde_json::to_value(&request)
                            .map_err(|_| Problem::new("invalid_request"))?,
                    )
                    .await?
            };
            if result.environment_id != id
                || result.project_id != request.project_id
                || result.worktree_id != request.worktree_id
                || result.mode != request.mode
                || result.base_ref != request.base_ref
                || result.context_lines != request.context_lines
                || (result.selected_file.as_ref().map(|f| f.file_id) != request.file_id
                    && !(result.selected_file.is_none()
                        && result.files.is_empty()
                        && result.state != "ready"))
            {
                return Err(Problem::new("identity_mismatch"));
            }
            Ok(result)
        })
        .await
        .map_err(|_| domain_problem("git_timeout"))?
    }
    pub async fn new() -> Self {
        let workspace = crate::workspace::WorkspaceStore::open().map_err(|_| Problem {
            code: "workspace_unavailable".into(),
            message:
                "Workspace references could not be loaded or locked; existing file is preserved."
                    .into(),
        });
        let states = workspace
            .as_ref()
            .map(|s| {
                s.value
                    .environments
                    .iter()
                    .map(|r| {
                        (
                            r.environment.id,
                            RuntimeState {
                                environment: r.environment.clone(),
                                state: "disconnected".into(),
                                error: Some(Problem::new("disconnected")),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let local = tokio::task::spawn_blocking(|| {
            default_config_path()
                .ok()
                .and_then(|p| Registry::open(p).ok())
                .map(|registry| {
                    let mut service = WorkspaceService::new(registry);
                    // Fixed bundled console executable; GUI-subsystem executables do not attach to ConPTY.
                    if let Ok(host) = std::env::current_exe() {
                        service.terminals =
                            agent_workspace_backend::terminal::TerminalService::with_executable(
                                host.with_file_name("terminal-host-windows.exe"),
                            );
                    }
                    service
                })
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| Problem::new("local_unavailable"));
        Self {
            workspace: Mutex::new(workspace),
            local,
            workers: Mutex::new(HashMap::new()),
            states: Mutex::new(states),
            restarted: Mutex::new(HashSet::new()),
            monitor: Mutex::new(()),
            stopping: Mutex::new(Vec::new()),
            launch: Mutex::new(()),
            requests: tokio::sync::Semaphore::new(4),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .expect("HTTP client"),
            closing: false.into(),
        }
    }
    pub fn local_environment(&self) -> Result<ExecutionEnvironment, Problem> {
        Ok(self
            .local
            .as_ref()
            .map_err(Clone::clone)?
            .environment
            .clone())
    }
    async fn publish(
        &self,
        app: &tauri::AppHandle,
        environment: ExecutionEnvironment,
        state: &str,
        error: Option<Problem>,
    ) {
        let event = RuntimeState {
            environment,
            state: state.into(),
            error,
        };
        self.states
            .lock()
            .await
            .insert(event.environment.id, event.clone());
        let _ = app.emit("runtime-state", event);
    }
    pub async fn states(&self) -> Vec<RuntimeState> {
        self.states.lock().await.values().cloned().collect()
    }
    pub async fn workspace(&self) -> Result<crate::workspace::Workspace, Problem> {
        self.workspace
            .lock()
            .await
            .as_ref()
            .map(|s| s.value.clone())
            .map_err(Clone::clone)
    }
    pub async fn restore(&self, app: &tauri::AppHandle) {
        let Ok(settings) = self.workspace().await else {
            return;
        };
        // Each failure is isolated; Windows refresh runs independently during restore.
        let futures = settings.environments.iter().map(|r| {
            self.connect(
                app,
                r.environment.distro.clone().unwrap_or_default(),
                Some(r.environment.id),
            )
        });
        for future in futures {
            let _ = future.await;
        }
    }
    pub async fn save_ui(
        &self,
        selection: Option<crate::workspace::Selection>,
        layout: crate::workspace::Layout,
    ) -> Result<(), Problem> {
        if let Some(s) = &selection {
            if !self
                .local_environment()
                .is_ok_and(|e| e.id == s.environment_id)
                && !self
                    .workspace()
                    .await?
                    .environments
                    .iter()
                    .any(|e| e.environment.id == s.environment_id)
            {
                return Err(Problem::new("unknown_environment"));
            }
        }
        let mut guard = self.workspace.lock().await;
        let store = guard.as_mut().map_err(|e| e.clone())?;
        let mut next = store.value.clone();
        next.selection = selection;
        next.layout = layout;
        store
            .save(next)
            .map_err(|_| Problem::new("operation_failed"))
    }
    pub async fn connect(
        &self,
        app: &tauri::AppHandle,
        distro: String,
        expected: Option<Uuid>,
    ) -> Result<ExecutionEnvironment, Problem> {
        let _launch = self.launch.lock().await;
        if self.closing.load(std::sync::atomic::Ordering::SeqCst)
            || distro.is_empty()
            || distro.len() > 128
            || distro.starts_with('-')
            || distro.chars().any(|c| c.is_control())
        {
            return Err(Problem::new("invalid_request"));
        }
        let settings = self.workspace().await?;
        let reference = settings
            .environments
            .iter()
            .find(|r| r.environment.distro.as_deref() == Some(&distro));
        let registration = match registration_id(&distro) {
            Ok(value) => value,
            Err(error) => {
                if let Some(r) = reference {
                    self.publish(app, r.environment.clone(), "error", Some(error.clone()))
                        .await;
                }
                return Err(error);
            }
        };
        if reference.is_some_and(|r| r.registration_id != registration) {
            let error = Problem::new("identity_mismatch");
            self.publish(
                app,
                reference.unwrap().environment.clone(),
                "error",
                Some(error.clone()),
            )
            .await;
            return Err(error);
        }
        let known = self
            .states
            .lock()
            .await
            .values()
            .find(|s| s.environment.distro.as_deref() == Some(&distro))
            .map(|s| s.environment.clone());
        if let Some(env) = &known {
            if expected.is_some_and(|id| id != env.id) {
                return Err(Problem::new("identity_mismatch"));
            }
            if self.workers.lock().await.contains_key(&env.id) {
                return Ok(env.clone());
            }
        }
        if known.is_none() && self.states.lock().await.len() >= 4 {
            return Err(Problem::new("runtime_limit"));
        }
        let result = self
            .start(&distro, expected.or(known.as_ref().map(|e| e.id)))
            .await;
        match result {
            Ok(worker) => {
                let env = worker.environment.clone();
                if !registration_id(&distro).is_ok_and(|id| id == registration) {
                    worker.stop().await;
                    return Err(Problem::new("identity_mismatch"));
                }
                let mut guard = self.workspace.lock().await;
                let store = guard.as_mut().map_err(|e| e.clone())?;
                let mut next = store.value.clone();
                if !next.environments.iter().any(|r| r.environment.id == env.id) {
                    next.environments
                        .push(crate::workspace::EnvironmentReference {
                            environment: env.clone(),
                            registration_id: registration,
                        });
                    if store.save(next).is_err() {
                        drop(guard);
                        worker.stop().await;
                        return Err(Problem::new("operation_failed"));
                    }
                }
                drop(guard);
                self.workers.lock().await.insert(env.id, worker);
                self.publish(app, env.clone(), "connected", None).await;
                Ok(env)
            }
            Err(error) => {
                if let Some(env) = known {
                    self.publish(app, env, "error", Some(error.clone())).await;
                }
                Err(error)
            }
        }
    }
    /// Explicit user action only. A changed identity never transfers project/selection IDs.
    pub async fn relink(
        &self,
        app: &tauri::AppHandle,
        old: Uuid,
    ) -> Result<ExecutionEnvironment, Problem> {
        let _launch = self.launch.lock().await;
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Problem::new("disconnected"));
        }
        let settings = self.workspace().await?;
        let reference = settings
            .environments
            .iter()
            .find(|r| r.environment.id == old)
            .ok_or_else(|| Problem::new("unknown_environment"))?;
        let distro = reference
            .environment
            .distro
            .as_deref()
            .ok_or_else(|| Problem::new("invalid_request"))?;
        let registration = registration_id(distro)?;
        if let Some(worker) = self.workers.lock().await.remove(&old) {
            worker.stop().await;
        }
        let worker = self.start(distro, None).await?;
        if !registration_id(distro).is_ok_and(|id| id == registration) {
            worker.stop().await;
            return Err(Problem::new("identity_mismatch"));
        }
        let env = worker.environment.clone();
        let mut guard = self.workspace.lock().await;
        let store = guard.as_mut().map_err(|e| e.clone())?;
        let mut next = store.value.clone();
        next.environments.retain(|r| r.environment.id != old);
        next.environments
            .push(crate::workspace::EnvironmentReference {
                environment: env.clone(),
                registration_id: registration,
            });
        if next
            .selection
            .as_ref()
            .is_some_and(|s| s.environment_id == old)
        {
            next.selection = None;
        }
        next.layout
            .collapsed
            .retain(|s| !s.contains(&old.to_string()));
        if store.save(next).is_err() {
            drop(guard);
            worker.stop().await;
            return Err(Problem::new("operation_failed"));
        }
        drop(guard);
        self.states.lock().await.remove(&old);
        self.workers.lock().await.insert(env.id, worker);
        self.publish(app, env.clone(), "connected", None).await;
        Ok(env)
    }
    pub async fn remove_project(&self, id: Uuid, project: Uuid) -> Result<(), Problem> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            if self.local_environment().is_ok_and(|e| e.id == id) {
                self.local
                    .as_ref()
                    .map_err(Clone::clone)?
                    .remove(project)
                    .await
                    .map_err(registry_problem)?;
            } else {
                let peer = self.peer(id).await?;
                peer.send_empty(
                    self.client
                        .delete(format!(
                            "http://127.0.0.1:{}/api/v1/projects/{project}?environment_id={id}",
                            peer.port
                        ))
                        .bearer_auth(&peer.token),
                )
                .await?;
            }
            let mut guard = self.workspace.lock().await;
            let store = guard.as_mut().map_err(|e| e.clone())?;
            let mut next = store.value.clone();
            if next
                .selection
                .as_ref()
                .is_some_and(|s| s.environment_id == id && s.project_id == project)
            {
                next.selection = None;
            }
            next.layout
                .collapsed
                .retain(|s| s != &format!("repo:{id}:{project}"));
            store
                .save(next)
                .map_err(|_| Problem::new("operation_failed"))
        })
        .await
        .map_err(|_| Problem::new("disconnected"))?
    }
    async fn start(&self, distro: &str, expected: Option<Uuid>) -> Result<Worker, Problem> {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut command = Command::new("wsl.exe");
        command
            .args([
                "--distribution",
                distro,
                "--exec",
                "/usr/bin/python3",
                "-c",
                BOOTSTRAP,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.env_remove("AWM_TOKEN");
        #[cfg(windows)]
        command.creation_flags(0x00000004 | 0x08000000); // suspended, no console
        let mut child = command
            .spawn()
            .map_err(|_| Problem::new("startup_failed"))?;
        #[cfg(windows)]
        let job = {
            let job = agent_workspace_backend::git::windows::Job::new()
                .map_err(|_| Problem::new("startup_failed"))?;
            job.assign_and_resume(&child)
                .map_err(|_| Problem::new("startup_failed"))?;
            job
        };
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Problem::new("startup_failed"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Problem::new("startup_failed"))?;
        let startup = Startup {
            token: token.clone(),
            nonce: nonce.clone(),
            protocol: PROTOCOL,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            exchange_bootstrap(&mut stdin, &mut stdout, &startup),
        )
        .await
        .map_err(|_| Problem::new("startup_failed"))
        .and_then(|r| r);
        let ready = match result {
            Ok(r) => r,
            Err(e) => {
                drop(stdin);
                let _ = child.kill().await;
                return Err(e);
            }
        };
        let provisional = ExecutionEnvironment {
            id: Uuid::nil(),
            kind: EnvironmentKind::Wsl,
            distro: Some(distro.into()),
        };
        let mut worker = Worker {
            child,
            stdin: Some(stdin),
            #[cfg(windows)]
            _job: job,
            peer: Peer {
                port: ready.port,
                token,
                nonce,
                environment: provisional,
            },
        };
        let peer = worker.peer.clone();
        let result = async {
            let h: Handshake = peer.request(&self.client, "runtime/handshake").await?;
            if h.environment.kind != EnvironmentKind::Wsl
                || h.environment.distro.as_deref() != Some(distro)
                || h.environment.id.is_nil()
                || expected.is_some_and(|id| id != h.environment.id)
            {
                return Err(Problem::new("identity_mismatch"));
            }
            validate_handshake(&h, &peer.nonce, &h.environment)?;
            Ok(h.environment)
        }
        .await;
        match result {
            Ok(env) => {
                worker.peer.environment = env;
                Ok(worker)
            }
            Err(e) => {
                worker.stop().await;
                Err(e)
            }
        }
    }
    pub async fn projects(&self, id: Uuid) -> Result<Vec<ProjectDto>, Problem> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            self.projects_inner(id).await
        })
        .await
        .map_err(|_| Problem::new("disconnected"))?
    }
    pub async fn add_project(&self, id: Uuid, path: String) -> Result<ProjectDto, Problem> {
        if path.len() > 4096 || path.contains('\0') {
            return Err(Problem::new("invalid_request"));
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            let project = if self.local_environment().is_ok_and(|e| e.id == id) {
                self.local
                    .as_ref()
                    .map_err(Clone::clone)?
                    .add(agent_workspace_backend::projects::NewProject {
                        root_path: path,
                        name: None,
                        base_ref: None,
                    })
                    .await
                    .map_err(registry_problem)?
            } else {
                self.peer(id)
                    .await?
                    .post(
                        &self.client,
                        &format!("projects?environment_id={id}"),
                        serde_json::json!({"root_path":path}),
                    )
                    .await?
            };
            if project.environment_id != id {
                return Err(Problem::new("identity_mismatch"));
            }
            Ok(project)
        })
        .await
        .map_err(|_| Problem::new("disconnected"))?
    }
    pub async fn directories(
        &self,
        id: Uuid,
        request: agent_workspace_backend::directories::DirectoryRequest,
    ) -> Result<agent_workspace_backend::directories::DirectoryPage, Problem> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            let peer = self.peer(id).await?;
            let page: agent_workspace_backend::directories::DirectoryPage = peer
                .post(
                    &self.client,
                    &format!("directories?environment_id={id}"),
                    serde_json::to_value(request).map_err(|_| Problem::new("invalid_request"))?,
                )
                .await?;
            if page.environment_id != id {
                return Err(Problem::new("identity_mismatch"));
            }
            Ok(page)
        })
        .await
        .map_err(|_| Problem::new("disconnected"))?
    }
    async fn projects_inner(&self, id: Uuid) -> Result<Vec<ProjectDto>, Problem> {
        if self.local_environment().is_ok_and(|e| e.id == id) {
            return self
                .local
                .as_ref()
                .map_err(Clone::clone)?
                .projects()
                .await
                .map_err(|_| Problem::new("operation_failed"));
        }
        let worker = self.peer(id).await?;
        let projects: Vec<ProjectDto> = worker
            .request(&self.client, &format!("projects?environment_id={id}"))
            .await?;
        if projects.iter().any(|p| p.environment_id != id) {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(projects)
    }
    pub async fn worktrees(
        &self,
        id: Uuid,
        project: Uuid,
        refresh: bool,
    ) -> Result<Snapshot, Problem> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _permit = self
                .requests
                .acquire()
                .await
                .map_err(|_| Problem::new("disconnected"))?;
            self.worktrees_inner(id, project, refresh).await
        })
        .await
        .map_err(|_| Problem::new("disconnected"))?
    }
    async fn worktrees_inner(
        &self,
        id: Uuid,
        project: Uuid,
        refresh: bool,
    ) -> Result<Snapshot, Problem> {
        if self.local_environment().is_ok_and(|e| e.id == id) {
            let worktrees = &self.local.as_ref().map_err(Clone::clone)?.worktrees;
            return if refresh {
                worktrees.refresh(project).await
            } else {
                worktrees.list(project).await
            }
            .map_err(|_| Problem::new("operation_failed"));
        }
        let worker = self.peer(id).await?;
        let snapshot: Snapshot = if refresh {
            worker
                .post(
                    &self.client,
                    &format!("projects/{project}/worktrees/refresh?environment_id={id}"),
                    serde_json::json!({}),
                )
                .await?
        } else {
            worker
                .request(
                    &self.client,
                    &format!("projects/{project}/worktrees?environment_id={id}"),
                )
                .await?
        };
        if snapshot.environment_id != id
            || snapshot.project_id != project
            || snapshot
                .worktrees
                .iter()
                .any(|w| w.environment_id != id || w.project_id != project)
        {
            return Err(Problem::new("identity_mismatch"));
        }
        Ok(snapshot)
    }
    async fn peer(&self, id: Uuid) -> Result<Peer, Problem> {
        self.workers
            .lock()
            .await
            .get(&id)
            .map(|w| w.peer.clone())
            .ok_or_else(|| Problem::new("unknown_environment"))
    }
    pub async fn refresh_local(&self) {
        if let Ok(local) = &self.local {
            let _ = local.worktrees.refresh_all().await;
        }
    }
    pub async fn tick(self: &Arc<Self>, app: &tauri::AppHandle) {
        let _monitor = self.monitor.lock().await;
        if self.closing.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let ids: Vec<Uuid> = self.workers.lock().await.keys().copied().collect();
        let mut checks = tokio::task::JoinSet::new();
        for id in ids {
            let peer = {
                let mut workers = self.workers.lock().await;
                if let Some(w) = workers.get_mut(&id) {
                    Some((
                        w.peer.clone(),
                        w.child.try_wait().map_or(true, |s| s.is_some()),
                    ))
                } else {
                    None
                }
            };
            if let Some((peer, exited)) = peer {
                let client = self.client.clone();
                checks.spawn(async move {
                    let error = if exited {
                        Some(Problem::new("disconnected"))
                    } else {
                        tokio::time::timeout(Duration::from_secs(2), peer.verify(&client))
                            .await
                            .unwrap_or_else(|_| Err(Problem::new("disconnected")))
                            .err()
                    };
                    (id, peer.nonce, error)
                });
            }
        }
        while let Some(Ok((id, nonce, error))) = checks.join_next().await {
            if let Some(error) = error {
                let worker = {
                    let mut workers = self.workers.lock().await;
                    // A relink can replace this ID while the old peer check is pending.
                    // Only the exact checked launch may be removed or stopped.
                    if workers.get(&id).is_some_and(|w| w.nonce == nonce) {
                        workers.remove(&id)
                    } else {
                        None
                    }
                };
                if let Some(worker) = worker {
                    let env = worker.environment.clone();
                    self.publish(app, env.clone(), "disconnected", Some(error.clone()))
                        .await;
                    let restart =
                        error.code == "disconnected" && self.restarted.lock().await.insert(id);
                    let runtime = self.clone();
                    let app = app.clone();
                    let task = tokio::spawn(async move {
                        worker.stop().await;
                        if restart {
                            let _ = runtime
                                .connect(&app, env.distro.unwrap_or_default(), Some(id))
                                .await;
                        }
                    });
                    let mut tasks = self.stopping.lock().await;
                    tasks.retain(|t| !t.is_finished());
                    tasks.push(task);
                }
            }
        }
    }
    pub async fn memory_sample(&self) -> crate::memory::Record {
        let captured_unix_ms = crate::memory::unix_ms();
        let start = std::time::Instant::now();
        let terminals = self
            .local
            .as_ref()
            .map(|s| s.terminals.clone())
            .unwrap_or_default();
        let host_task = tokio::task::spawn_blocking(move || {
            agent_workspace_backend::memory::sample(&terminals)
        });
        // Connected peers only: monitoring never starts WSL or reconnects a distro.
        let peers: Vec<_> = self
            .workers
            .lock()
            .await
            .values()
            .map(|w| (w.environment.clone(), w.peer.clone()))
            .collect();
        let mut tasks = tokio::task::JoinSet::new();
        for (environment, peer) in peers {
            let client = self.client.clone();
            tasks.spawn(async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    peer.request::<agent_workspace_backend::memory::Snapshot>(
                        &client,
                        "runtime/memory",
                    ),
                )
                .await;
                let snapshot = match result {
                    Ok(Ok(s))
                        if s.platform == "linux"
                            && s.processes.len()
                                <= agent_workspace_backend::memory::MAX_PROCESSES
                            && s.terminals.len() <= 8
                            && s.processes.iter().all(|p| p.name.chars().count() <= 64) =>
                    {
                        Some(s)
                    }
                    _ => None,
                };
                crate::memory::EnvironmentSnapshot {
                    environment_id: environment.id,
                    distro: environment.distro,
                    error: snapshot
                        .is_none()
                        .then(|| "worker_memory_unavailable".into()),
                    snapshot,
                }
            });
        }
        let mut environments = vec![];
        while let Some(result) = tasks.join_next().await {
            if let Ok(sample) = result {
                environments.push(sample);
            }
        }
        environments.sort_by_key(|e| e.environment_id);
        let host = host_task.await.ok();
        let errors = if host.is_none() {
            vec!["host_collection_failed".into()]
        } else {
            vec![]
        };
        crate::memory::Record {
            record_type: "sample",
            schema_version: 1,
            captured_unix_ms,
            collection_duration_ms: start.elapsed().as_millis() as u64,
            host,
            environments,
            errors,
        }
    }
    pub async fn close(&self) {
        self.closing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _monitor = self.monitor.lock().await;
        let _launch = self.launch.lock().await;
        if let Ok(local) = &self.local {
            local.agents.close_all().await;
            let terminals = local.terminals.clone();
            let _ = tokio::task::spawn_blocking(move || terminals.close_all()).await;
        }
        let workers = std::mem::take(&mut *self.workers.lock().await);
        for (_, worker) in workers {
            worker.stop().await;
        }
        drop(_launch);
        for task in std::mem::take(&mut *self.stopping.lock().await) {
            let _ = task.await;
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub environment_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectScope {
    pub environment_id: Uuid,
    pub project_id: Uuid,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn early_bootstrap_refusal_survives_broken_asset_pipe() {
        let (mut stdin, reader) = tokio::io::duplex(128);
        let (mut writer, mut stdout) = tokio::io::duplex(128);
        drop(reader);
        writer
            .write_all(b"{\"error\":\"helper_prerequisite\"}\n")
            .await
            .unwrap();
        drop(writer);
        let startup = Startup {
            token: "a".repeat(64),
            nonce: "b".repeat(64),
            protocol: PROTOCOL,
        };
        let result = exchange_bootstrap(&mut stdin, &mut stdout, &startup).await;
        assert_eq!(result.err().unwrap().code, "helper_prerequisite");
    }
    #[test]
    fn bootstrap_failures_are_bounded_fixed_messages() {
        assert_eq!(bootstrap_reply(br#"{"port":1234}"#).unwrap().port, 1234);
        for code in [
            "helper_platform",
            "helper_prerequisite",
            "helper_temp",
            "helper_storage",
            "helper_asset",
        ] {
            let bytes = serde_json::to_vec(&serde_json::json!({"error":code})).unwrap();
            let error = bootstrap_reply(&bytes).err().unwrap();
            assert_eq!(error.code, code);
            assert!(!error.message.contains("Workspace operation failed"));
        }
        for bytes in [
            br#"{"port":0}"#.as_slice(),
            br#"{"error":"secret arbitrary diagnostic"}"#,
            b"not json",
        ] {
            let error = bootstrap_reply(bytes).err().unwrap();
            assert_eq!(error.code, "startup_failed");
            assert!(!error.message.contains("secret"));
        }
    }
    #[test]
    fn handshake_requires_version_nonce_and_full_identity() {
        let e = ExecutionEnvironment {
            id: Uuid::new_v4(),
            kind: EnvironmentKind::Wsl,
            distro: Some("Ubuntu".into()),
        };
        let mut h = Handshake {
            protocol: PROTOCOL,
            version: VERSION.into(),
            nonce: "private".into(),
            environment: e.clone(),
        };
        assert!(validate_handshake(&h, "private", &e).is_ok());
        assert!(validate_handshake(&h, "foreign", &e).is_err());
        h.version = "foreign".into();
        assert_eq!(
            validate_handshake(&h, "private", &e).unwrap_err().code,
            "incompatible_worker"
        );
        h.version = VERSION.into();
        h.environment.distro = Some("Other".into());
        assert!(validate_handshake(&h, "private", &e).is_err());
    }
}
