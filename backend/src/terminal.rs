//! Environment-local, worktree-scoped PTYs. Session IDs never authorize another scope.
use crate::service::WorkspaceService;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    sync::{mpsc, Arc, Mutex},
};
use uuid::Uuid;
const OUTPUT: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    #[serde(flatten)]
    pub action: Action,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    OpenIde {
        request_id: Uuid,
        executable: std::path::PathBuf,
    },
    OpenHarness {
        request_id: Uuid,
        rows: u16,
        cols: u16,
        installation_id: Uuid,
    },
    Open {
        request_id: Uuid,
        rows: u16,
        cols: u16,
    },
    List,
    Read {
        session_id: Uuid,
        cursor: u64,
    },
    Input {
        session_id: Uuid,
        data: String,
    },
    Resize {
        session_id: Uuid,
        rows: u16,
        cols: u16,
    },
    Close {
        session_id: Uuid,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub cwd: String,
    pub shell: String,
    pub state: String,
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub rows: u16,
    pub cols: u16,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub sessions: Vec<SessionInfo>,
    pub session_id: Option<Uuid>,
    pub data: Vec<u8>,
    pub cursor: u64,
    pub lost: bool,
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct TerminalError(pub &'static str);
pub fn message(code: &str) -> &'static str {
    match code {
        "terminal_active" => {
            "Закройте управляемые терминалы перед удалением регистрации repository."
        }
        "terminal_scope" => "Терминал/worktree недоступен или его identity изменилась.",
        "terminal_limit" => "Достигнут лимит терминалов (8). Закройте ненужные сессии.",
        "terminal_input" => "Некорректный ввод/размер терминала или очередь ввода заполнена.",
        "terminal_ide_executable" => "IDE executable недоступен: укажите абсолютный путь; Windows поддерживает .exe, WSL — исполняемый файл выбранной среды.",
        "terminal_ide_launch" => "IDE не запущена. Проверьте executable и его зависимости в выбранной среде.",
        _ => "PTY не запущен или операция терминала не завершена. Проверьте состояние сессии.",
    }
}
struct Buffer {
    bytes: VecDeque<u8>,
    end: u64,
    info: SessionInfo,
}
struct Session {
    buffer: Arc<Mutex<Buffer>>,
    master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
}
impl Session {
    fn observe(&mut self) -> SessionInfo {
        let mut b = self.buffer.lock().unwrap();
        if b.info.state == "running" {
            match self.child.try_wait() {
                Ok(Some(code)) => {
                    b.info.state = "exited".into();
                    b.info.exit_code = Some(code.exit_code());
                }
                Err(_) => {
                    b.info.state = "error".into();
                    b.info.error = Some("Не удалось проверить процесс".into());
                }
                _ => {}
            }
        }
        b.info.clone()
    }
    fn close(&mut self) -> Result<(), TerminalError> {
        if self.observe().state != "exited" {
            #[cfg(unix)]
            self.child
                .clone_killer()
                .kill()
                .map_err(|_| TerminalError("terminal_failed"))?;
            #[cfg(windows)]
            self.child
                .kill()
                .map_err(|_| TerminalError("terminal_failed"))?;
        }
        self.input.take();
        // Supervisor kills/reaps its own descendants before reporting exit.
        for _ in 0..100 {
            if self
                .child
                .try_wait()
                .map_err(|_| TerminalError("terminal_failed"))?
                .is_some()
            {
                self.observe();
                self.master.take();
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err(TerminalError("terminal_failed"))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
#[derive(Clone, Default)]
pub struct TerminalService {
    executable: Option<std::path::PathBuf>,
    sessions: Arc<Mutex<HashMap<Uuid, Session>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    ide_requests: Arc<Mutex<VecDeque<(Uuid, Uuid, Uuid)>>>,
}
impl TerminalService {
    pub fn memory_sessions(&self) -> Vec<crate::memory::TerminalMemory> {
        self.sessions
            .lock()
            .unwrap()
            .values_mut()
            .map(|session| {
                let info = session.observe();
                let buffer = session.buffer.lock().unwrap();
                crate::memory::TerminalMemory {
                    session_id: info.id,
                    project_id: info.project_id,
                    worktree_id: info.worktree_id,
                    supervisor_pid: session.child.process_id(),
                    state: info.state,
                    buffer_bytes: buffer.bytes.len(),
                    buffer_capacity_bytes: buffer.bytes.capacity(),
                }
            })
            .collect()
    }
    // External editors have independent lifetimes. Never attach them to a PTY,
    // supervisor, job or the app's terminal registry, including an existing IDE.
    fn open_ide(
        &self,
        environment: crate::environment::ExecutionEnvironment,
        request: Request,
        path: std::path::PathBuf,
        cwd: std::path::PathBuf,
    ) -> Result<Response, TerminalError> {
        use std::process::{Command, Stdio};
        let Action::OpenIde { request_id, .. } = request.action else {
            return Err(TerminalError("terminal_input"));
        };
        if request_id.is_nil() {
            return Err(TerminalError("terminal_input"));
        }
        // Share the close lock: no new external launch may begin after close_all.
        let _sessions = self.sessions.lock().unwrap();
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TerminalError("terminal_scope"));
        }
        let mut requests = self.ide_requests.lock().unwrap();
        if let Some((_, project, tree)) = requests.iter().find(|(id, _, _)| *id == request_id) {
            if *project != request.project_id || *tree != request.worktree_id {
                return Err(TerminalError("terminal_scope"));
            }
        } else {
            let mut command = Command::new(path);
            #[cfg(unix)]
            command.arg("--remote").arg(format!(
                "wsl+{}",
                environment
                    .distro
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .ok_or(TerminalError("terminal_scope"))?
            ));
            command
                .arg("--new-window")
                .arg(&cwd)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("GIT_")
                    || matches!(
                        key.to_str(),
                        Some("AWM_TOKEN" | "NODE_OPTIONS" | "NODE_PATH" | "ELECTRON_RUN_AS_NODE")
                    )
                {
                    command.env_remove(key);
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x00000008 | 0x00000200); // detached, independent process group
            }
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let mut child = command
                .spawn()
                .map_err(|_| TerminalError("terminal_ide_launch"))?;
            // Reap the launcher when it exits; retaining a wait handle grants no
            // cancellation authority and does not keep the app alive on exit.
            std::thread::Builder::new()
                .name("external-ide-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                })
                .map_err(|_| TerminalError("terminal_ide_launch"))?;
            requests.push_back((request_id, request.project_id, request.worktree_id));
            if requests.len() > 256 {
                requests.pop_front();
            }
        }
        Ok(Response {
            environment_id: environment.id,
            project_id: request.project_id,
            worktree_id: request.worktree_id,
            sessions: vec![],
            session_id: None,
            data: vec![],
            cursor: 0,
            lost: false,
        })
    }
    /// Trusted host configuration only; never accepted from IPC.
    pub fn with_executable(executable: std::path::PathBuf) -> Self {
        Self {
            executable: Some(executable),
            ..Default::default()
        }
    }
    pub fn active_project(&self, project: Uuid) -> bool {
        self.sessions.lock().unwrap().values_mut().any(|s| {
            let i = s.observe();
            i.project_id == project && i.state != "exited"
        })
    }
    pub fn active(&self, project: Uuid, tree: Uuid) -> bool {
        self.sessions.lock().unwrap().values_mut().any(|s| {
            let i = s.observe();
            i.project_id == project && i.worktree_id == tree && i.state != "exited"
        })
    }
    pub fn close_all(&self) {
        let mut sessions = self.sessions.lock().unwrap();
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        sessions.clear();
    }
    pub fn forget(&self, project: Uuid, tree: Uuid) {
        self.sessions.lock().unwrap().retain(|_, s| {
            let i = s.observe();
            i.project_id != project || i.worktree_id != tree
        });
    }
    fn execute(
        &self,
        env: Uuid,
        request: Request,
        cwd: Option<String>,
        executable: std::path::PathBuf,
    ) -> Result<Response, TerminalError> {
        self.execute_target(env, request, cwd, executable, None)
    }
    fn execute_target(
        &self,
        env: Uuid,
        request: Request,
        cwd: Option<String>,
        executable: std::path::PathBuf,
        target: Option<(std::path::PathBuf, Vec<String>)>,
    ) -> Result<Response, TerminalError> {
        let mut sessions = self.sessions.lock().unwrap();
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TerminalError("terminal_scope"));
        }
        let Request {
            project_id,
            worktree_id,
            action,
        } = request;
        let mut response = Response {
            environment_id: env,
            project_id,
            worktree_id,
            sessions: vec![],
            session_id: None,
            data: vec![],
            cursor: 0,
            lost: false,
        };
        if let Action::Open {
            request_id,
            rows,
            cols,
        } = action
        {
            size(rows, cols)?;
            if request_id.is_nil() {
                return Err(TerminalError("terminal_input"));
            }
            if let Some(s) = sessions.get(&request_id) {
                let b = s.buffer.lock().unwrap();
                if b.info.project_id != project_id || b.info.worktree_id != worktree_id {
                    return Err(TerminalError("terminal_scope"));
                }
            } else {
                if sessions.len() >= 8 {
                    return Err(TerminalError("terminal_limit"));
                }
                let cwd = cwd.ok_or(TerminalError("terminal_scope"))?;
                let pair = native_pty_system()
                    .openpty(size(rows, cols)?)
                    .map_err(|_| TerminalError("terminal_failed"))?;
                let mut command = CommandBuilder::new(executable);
                command.arg(if target.is_some() {
                    "--harness-stdio"
                } else {
                    "--terminal-shell"
                });
                command.arg(std::process::id().to_string());
                if let Some((path, args)) = &target {
                    command.arg(path);
                    for arg in args {
                        command.arg(arg);
                    }
                }
                command.cwd(&cwd);
                command.env("TERM", "xterm-256color");
                if let Some((node, args)) = &target {
                    if args
                        .first()
                        .is_some_and(|a| a.ends_with(".js") || a.ends_with(".mjs"))
                    {
                        let mut paths = vec![node
                            .parent()
                            .ok_or(TerminalError("terminal_scope"))?
                            .to_path_buf()];
                        paths.extend(std::env::split_paths(
                            &std::env::var_os("PATH").unwrap_or_default(),
                        ));
                        command.env(
                            "PATH",
                            std::env::join_paths(paths)
                                .map_err(|_| TerminalError("terminal_scope"))?,
                        );
                        command.env_remove("NODE_OPTIONS");
                        command.env_remove("NODE_PATH");
                    }
                }
                for (key, _) in std::env::vars_os() {
                    if key.to_string_lossy().starts_with("GIT_") || key == "AWM_TOKEN" {
                        command.env_remove(key);
                    }
                }
                let mut reader = pair
                    .master
                    .try_clone_reader()
                    .map_err(|_| TerminalError("terminal_failed"))?;
                let mut writer = pair
                    .master
                    .take_writer()
                    .map_err(|_| TerminalError("terminal_failed"))?;
                let child = pair
                    .slave
                    .spawn_command(command)
                    .map_err(|_| TerminalError("terminal_failed"))?;
                drop(pair.slave);
                let buffer = Arc::new(Mutex::new(Buffer {
                    bytes: VecDeque::new(),
                    end: 0,
                    info: SessionInfo {
                        id: request_id,
                        project_id,
                        worktree_id,
                        cwd,
                        shell: if let Some((path, _)) = &target {
                            path.file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        } else if cfg!(windows) {
                            "Windows PowerShell".to_owned()
                        } else {
                            "Bash".to_owned()
                        },
                        state: "running".into(),
                        exit_code: None,
                        error: None,
                        rows,
                        cols,
                    },
                }));
                let output = buffer.clone();
                std::thread::spawn(move || {
                    let mut bytes = [0; 8192];
                    loop {
                        match reader.read(&mut bytes) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let mut b = output.lock().unwrap();
                                b.bytes.extend(&bytes[..n]);
                                b.end += n as u64;
                                let remove = b.bytes.len().saturating_sub(OUTPUT);
                                b.bytes.drain(..remove);
                            }
                        }
                    }
                });
                let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
                let errors = buffer.clone();
                std::thread::spawn(move || {
                    while let Ok(data) = rx.recv() {
                        if writer
                            .write_all(&data)
                            .and_then(|_| writer.flush())
                            .is_err()
                        {
                            errors.lock().unwrap().info.error = Some("Ошибка PTY input".into());
                            break;
                        }
                    }
                });
                sessions.insert(
                    request_id,
                    Session {
                        buffer,
                        master: Some(pair.master),
                        child,
                        input: Some(tx),
                    },
                );
            }
            response.session_id = Some(request_id);
        } else if !matches!(action, Action::List) {
            let id = match &action {
                Action::Read { session_id, .. }
                | Action::Input { session_id, .. }
                | Action::Resize { session_id, .. }
                | Action::Close { session_id } => *session_id,
                _ => unreachable!(),
            };
            let session = sessions
                .get_mut(&id)
                .ok_or(TerminalError("terminal_scope"))?;
            let info = session.observe();
            if info.project_id != project_id || info.worktree_id != worktree_id {
                return Err(TerminalError("terminal_scope"));
            }
            response.session_id = Some(id);
            match action {
                Action::Read { cursor, .. } => {
                    let b = session.buffer.lock().unwrap();
                    let start = b.end - b.bytes.len() as u64;
                    let next = cursor.max(start).min(b.end);
                    response.lost = cursor < start || cursor > b.end;
                    response.data = b
                        .bytes
                        .iter()
                        .skip((next - start) as usize)
                        .take(32 * 1024)
                        .copied()
                        .collect();
                    response.cursor = next + response.data.len() as u64;
                }
                Action::Input { data, .. } => {
                    if data.len() > 8192 || info.state != "running" {
                        return Err(TerminalError("terminal_input"));
                    }
                    session
                        .input
                        .as_ref()
                        .ok_or(TerminalError("terminal_input"))?
                        .try_send(data.into_bytes())
                        .map_err(|_| TerminalError("terminal_input"))?;
                }
                Action::Resize { rows, cols, .. } => {
                    session
                        .master
                        .as_ref()
                        .ok_or(TerminalError("terminal_scope"))?
                        .resize(size(rows, cols)?)
                        .map_err(|_| TerminalError("terminal_failed"))?;
                    let mut b = session.buffer.lock().unwrap();
                    b.info.rows = rows;
                    b.info.cols = cols;
                }
                Action::Close { .. } => {
                    session.close()?;
                    sessions.remove(&id);
                }
                _ => unreachable!(),
            }
        }
        response.sessions = sessions
            .values_mut()
            .map(Session::observe)
            .filter(|s| s.project_id == project_id && s.worktree_id == worktree_id)
            .collect();
        response.sessions.sort_by_key(|s| s.id);
        Ok(response)
    }
}
fn size(rows: u16, cols: u16) -> Result<PtySize, TerminalError> {
    if !(2..=300).contains(&rows) || !(2..=500).contains(&cols) {
        return Err(TerminalError("terminal_input"));
    }
    Ok(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })
}
pub async fn execute(
    service: &WorkspaceService,
    mut request: Request,
) -> Result<Response, TerminalError> {
    // Launch must exclude worktree removal. Existing-session IO already checks
    // its recorded scope and must not wait behind unrelated Git mutations.
    let _write = if matches!(
        request.action,
        Action::Open { .. } | Action::OpenHarness { .. } | Action::OpenIde { .. }
    ) {
        Some(service.create_lock.lock().await)
    } else {
        None
    };
    let target = if let Action::OpenHarness {
        request_id,
        rows,
        cols,
        installation_id,
    } = request.action
    {
        let i = service
            .harnesses
            .get(installation_id)
            .map_err(|_| TerminalError("terminal_scope"))?;
        let (path, args) = if let Some(npm) = &i.npm {
            crate::acp::npm_version(npm)
                .await
                .map_err(|_| TerminalError("terminal_scope"))?;
            (
                npm.node.clone(),
                vec![npm
                    .entry
                    .to_str()
                    .ok_or(TerminalError("terminal_scope"))?
                    .into()],
            )
        } else {
            (
                crate::harness::executable(&i.executable)
                    .map_err(|_| TerminalError("terminal_scope"))?,
                vec![],
            )
        };
        crate::harness::directory(service, request.project_id, request.worktree_id)
            .await
            .map_err(|_| TerminalError("terminal_scope"))?;
        request.action = Action::Open {
            request_id,
            rows,
            cols,
        };
        Some((path, args))
    } else if let Action::OpenIde {
        request_id: _,
        ref executable,
    } = request.action
    {
        if executable.as_os_str().len() > 4096 {
            return Err(TerminalError("terminal_input"));
        }
        let path = crate::harness::executable(executable)
            .map_err(|_| TerminalError("terminal_ide_executable"))?;
        let cwd = crate::harness::directory(service, request.project_id, request.worktree_id)
            .await
            .map_err(|_| TerminalError("terminal_scope"))?;
        let terminals = service.terminals.clone();
        let environment = service.environment.clone();
        return tokio::task::spawn_blocking(move || {
            terminals.open_ide(environment, request, path, cwd)
        })
        .await
        .map_err(|_| TerminalError("terminal_ide_launch"))?;
    } else {
        None
    };
    let cwd = if matches!(request.action, Action::Open { .. }) {
        let tree = service
            .worktrees
            .get(request.worktree_id)
            .await
            .map_err(|_| TerminalError("terminal_scope"))?;
        if tree.project_id != request.project_id
            || !tree.is_available
            || tree.is_bare
            || tree.availability_error.is_some()
        {
            return Err(TerminalError("terminal_scope"));
        }
        Some(
            tree.path
                .to_str()
                .ok_or(TerminalError("terminal_scope"))?
                .to_owned(),
        )
    } else {
        None
    };
    let terminals = service.terminals.clone();
    let env = service.environment.id;
    let executable = match &terminals.executable {
        Some(path) => path.clone(),
        None => std::env::current_exe().map_err(|_| TerminalError("terminal_failed"))?,
    };
    tokio::task::spawn_blocking(move || {
        if target.is_some() {
            terminals.execute_target(env, request, cwd, executable, target)
        } else {
            terminals.execute(env, request, cwd, executable)
        }
    })
    .await
    .map_err(|_| TerminalError("terminal_failed"))?
}

#[cfg(test)]
mod responsiveness_tests {
    use super::*;

    #[tokio::test]
    async fn existing_session_operations_do_not_wait_for_git_mutations() {
        let root = tempfile::tempdir().unwrap();
        let service = WorkspaceService::new(
            crate::projects::Registry::open(root.path().join("config.json")).unwrap(),
        );
        let _mutation = service.create_lock.lock().await;
        let project_id = Uuid::new_v4();
        let worktree_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        for action in [
            Action::List,
            Action::Read {
                session_id,
                cursor: 0,
            },
            Action::Input {
                session_id,
                data: "x".into(),
            },
            Action::Resize {
                session_id,
                rows: 24,
                cols: 80,
            },
            Action::Close { session_id },
        ] {
            let request = Request {
                project_id,
                worktree_id,
                action,
            };
            // Scope errors are expected for the nonexistent session; waiting for
            // the unrelated mutation is the regression under test.
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                execute(&service, request),
            )
            .await
            .expect("terminal IO blocked by Git mutation")
            .ok();
        }
        let request = Request {
            project_id,
            worktree_id,
            action: Action::Open {
                request_id: session_id,
                rows: 24,
                cols: 80,
            },
        };
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(30),
            execute(&service, request)
        )
        .await
        .is_err());
    }
}
