//! One ACP v1 newline JSON-RPC client. No shell, credential storage, or filesystem client capability.
use crate::{
    harness::{directory, executable, npm, Error, Protocol},
    service::WorkspaceService,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
};
use uuid::Uuid;
const FRAME: usize = 64 * 1024;
const EVENTS: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum Activity {
    Unknown,
    Working,
    Idle,
    Waiting,
    Error,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Event {
    pub sequence: u64,
    pub session_id: Option<String>,
    pub kind: String,
    pub text: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Capabilities {
    pub load: bool,
    pub list: bool,
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub current_value: String,
    pub options: Vec<Choice>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Permission {
    #[serde(skip)]
    rpc_id: Value,
    pub id: u64,
    pub session_id: String,
    pub title: String,
    pub options: Vec<Choice>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub directory: Option<PathBuf>,
    pub assigned: bool,
    pub activity: Activity,
    pub modes: Vec<Choice>,
    pub current_mode: Option<String>,
    #[serde(default)]
    pub config_options: Vec<ConfigOption>,
    #[serde(default)]
    pub commands: Vec<AvailableCommand>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Instance {
    pub id: Uuid,
    pub environment_id: Uuid,
    pub installation_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub directory: PathBuf,
    pub connection: String,
    pub stale: bool,
    pub error: Option<String>,
    pub owned: bool,
    pub protocol_version: Option<u64>,
    pub capabilities: Capabilities,
    pub sessions: Vec<Session>,
    pub permissions: Vec<Permission>,
    pub events: VecDeque<Event>,
    pub cursor: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Attach {
        port: u16,
        instance_id: Uuid,
    },
    Launch {
        installation_id: Uuid,
        instance_id: Uuid,
        #[serde(default)]
        candidate: bool,
    },
    List,
    Stop {
        instance_id: Uuid,
    },
    New {
        instance_id: Uuid,
    },
    Prompt {
        instance_id: Uuid,
        session_id: String,
        text: String,
    },
    Cancel {
        instance_id: Uuid,
        session_id: String,
    },
    Permission {
        instance_id: Uuid,
        permission_id: u64,
        option_id: Option<String>,
    },
    Mode {
        instance_id: Uuid,
        session_id: String,
        mode_id: String,
    },
    Config {
        instance_id: Uuid,
        session_id: String,
        config_id: String,
        value: String,
    },
    History {
        instance_id: Uuid,
    },
    Load {
        instance_id: Uuid,
        session_id: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub request_id: Uuid,
    #[serde(flatten)]
    pub action: Action,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub request_id: Uuid,
    pub instances: Vec<Instance>,
}
struct Handle {
    state: Arc<Mutex<Instance>>,
    tx: mpsc::Sender<Action>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Clone)]
pub struct Client {
    handles: Arc<Mutex<HashMap<Uuid, Handle>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    requests: Arc<tokio::sync::Semaphore>,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            handles: Default::default(),
            closed: Default::default(),
            requests: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}
impl Client {
    pub fn active(&self, project: Uuid, tree: Option<Uuid>) -> bool {
        self.handles.lock().unwrap().values().any(|h| {
            let s = h.state.lock().unwrap();
            s.project_id == project
                && tree.is_none_or(|t| t == s.worktree_id)
                && !h.task.is_finished()
        })
    }
    pub async fn close_all(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        let handles = std::mem::take(&mut *self.handles.lock().unwrap());
        for (id, h) in handles {
            let _ = h.tx.send(Action::Stop { instance_id: id }).await;
            let _ = h.task.await;
        }
    }
    pub async fn execute(&self, service: &WorkspaceService, r: Request) -> Result<Response, Error> {
        tokio::time::timeout(Duration::from_secs(12), self.execute_bounded(service, r))
            .await
            .map_err(|_| Error("harness_timeout"))?
    }
    async fn execute_bounded(
        &self,
        service: &WorkspaceService,
        r: Request,
    ) -> Result<Response, Error> {
        let bypass = matches!(
            r.action,
            Action::Stop { .. } | Action::Cancel { .. } | Action::Permission { .. }
        );
        let _permit = if bypass {
            None
        } else {
            Some(
                self.requests
                    .try_acquire()
                    .map_err(|_| Error("harness_busy"))?,
            )
        };
        if r.request_id.is_nil() || self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error("harness_scope"));
        }
        // List/stop/cancel remain usable even if the worktree has disappeared.
        if let Action::Attach { port, instance_id } = r.action {
            if port == 0 || instance_id.is_nil() {
                return Err(Error("harness_scope"));
            }
            let cwd = directory(service, r.project_id, r.worktree_id).await?;
            let mut handles = self.handles.lock().unwrap();
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Error("harness_scope"));
            }
            if handles.len() >= 8 {
                handles.retain(|_, h| !h.task.is_finished());
            }
            if handles.len() >= 8 || handles.contains_key(&instance_id) {
                return Err(Error("harness_limit"));
            }
            let state = Arc::new(Mutex::new(Instance {
                id: instance_id,
                environment_id: service.environment.id,
                installation_id: Uuid::nil(),
                project_id: r.project_id,
                worktree_id: r.worktree_id,
                directory: cwd,
                connection: "connecting".into(),
                stale: true,
                error: None,
                owned: false,
                protocol_version: None,
                capabilities: Capabilities::default(),
                sessions: vec![],
                permissions: vec![],
                events: VecDeque::new(),
                cursor: 0,
            }));
            let (tx, rx) = mpsc::channel(8);
            let copy = state.clone();
            let service = service.clone();
            let task = tokio::spawn(async move {
                crate::provider::monitor(service, port, copy, rx).await;
            });
            handles.insert(instance_id, Handle { state, tx, task });
        } else if let Action::Launch {
            installation_id,
            instance_id,
            candidate,
        } = r.action
        {
            let _write = service.create_lock.lock().await;
            let cwd = directory(service, r.project_id, r.worktree_id).await?;
            let i = service.harnesses.get(installation_id)?;
            if (i.protocol != Protocol::Acp && !(candidate && i.npm.is_some()))
                || instance_id.is_nil()
            {
                return Err(Error("harness_incompatible"));
            }
            // Reprobe prevents replacement/version drift between discovery and launch.
            let actual = if let Some(npm) = &i.npm {
                npm_version(npm).await?
            } else {
                version(&executable(&i.executable)?).await?
            };
            if actual.as_str() != i.version.as_deref().unwrap_or("") {
                return Err(Error("harness_incompatible"));
            }
            let mut handles = self.handles.lock().unwrap();
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Error("harness_scope"));
            }
            if handles.contains_key(&instance_id) {
                return Err(Error("harness_scope"));
            }
            if handles.len() >= 8 {
                handles.retain(|_, h| !h.task.is_finished());
            }
            if handles.len() >= 8 {
                return Err(Error("harness_limit"));
            }
            let state = Arc::new(Mutex::new(Instance {
                id: instance_id,
                environment_id: service.environment.id,
                installation_id,
                project_id: r.project_id,
                worktree_id: r.worktree_id,
                directory: cwd.clone(),
                connection: "connecting".into(),
                stale: true,
                error: None,
                owned: true,
                protocol_version: None,
                capabilities: Capabilities::default(),
                sessions: vec![],
                permissions: vec![],
                events: VecDeque::new(),
                cursor: 0,
            }));
            let descriptor =
                crate::harness::descriptor(&i.name).ok_or(Error("harness_incompatible"))?;
            let child = if let Some(npm) = &i.npm {
                spawn_npm(npm, descriptor.argv, &cwd)?
            } else {
                spawn(&executable(&i.executable)?, descriptor.argv, &cwd, None)?
            };
            let (tx, rx) = mpsc::channel(8);
            let copy = state.clone();
            let task = tokio::spawn(async move {
                run(child, copy, rx).await;
            });
            handles.insert(instance_id, Handle { state, tx, task });
        } else if !matches!(r.action, Action::List) {
            let id = match &r.action {
                Action::Stop { instance_id }
                | Action::New { instance_id }
                | Action::Prompt { instance_id, .. }
                | Action::Cancel { instance_id, .. }
                | Action::Permission { instance_id, .. }
                | Action::Mode { instance_id, .. }
                | Action::Config { instance_id, .. }
                | Action::History { instance_id }
                | Action::Load { instance_id, .. } => *instance_id,
                _ => unreachable!(),
            };
            let safe = matches!(
                r.action,
                Action::Stop { .. }
                    | Action::Cancel { .. }
                    | Action::Permission {
                        option_id: None,
                        ..
                    }
            );
            let (tx, cwd) = {
                let handles = self.handles.lock().unwrap();
                let h = handles.get(&id).ok_or(Error("harness_scope"))?;
                let s = h.state.lock().unwrap();
                if s.project_id != r.project_id || s.worktree_id != r.worktree_id {
                    return Err(Error("harness_scope"));
                }
                if !s.owned && !matches!(r.action, Action::Stop { .. }) {
                    return Err(Error("harness_incompatible"));
                }
                (h.tx.clone(), s.directory.clone())
            };
            if !safe && directory(service, r.project_id, r.worktree_id).await? != cwd {
                return Err(Error("harness_scope"));
            }
            tx.try_send(r.action.clone())
                .map_err(|_| Error("harness_busy"))?;
        }
        let instances = self
            .handles
            .lock()
            .unwrap()
            .values()
            .filter_map(|h| {
                let s = h.state.lock().unwrap();
                (s.project_id == r.project_id && s.worktree_id == r.worktree_id).then(|| s.clone())
            })
            .collect();
        Ok(Response {
            environment_id: service.environment.id,
            project_id: r.project_id,
            worktree_id: r.worktree_id,
            request_id: r.request_id,
            instances,
        })
    }
}

struct Owned {
    child: Option<Child>,
    #[cfg(windows)]
    job: Option<crate::git::windows::Job>,
}
impl Drop for Owned {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
            #[cfg(windows)]
            let _ = child.start_kill();
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    if tokio::time::timeout(Duration::from_secs(2), child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                    }
                });
            }
        }
    }
}
fn spawn(path: &Path, args: &[&str], cwd: &Path, node: Option<&Path>) -> Result<Owned, Error> {
    let supervisor = if cfg!(windows) {
        std::env::current_exe()
            .map_err(|_| Error("harness_disconnect"))?
            .with_file_name("terminal-host-windows.exe")
    } else {
        std::env::current_exe().map_err(|_| Error("harness_disconnect"))?
    };
    let mut cmd = Command::new(supervisor);
    cmd.arg("--harness-stdio")
        .arg(std::process::id().to_string())
        .arg(path)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if args.first() == Some(&"acp") && std::env::var_os("OPENCODE_SERVER_PASSWORD").is_none() {
        // Ephemeral internal HTTP authorization, consumed only by the owned harness.
        cmd.env("OPENCODE_SERVER_PASSWORD", Uuid::new_v4().to_string());
    }
    if let Some(node) = node {
        // SDKs may start their own Node child; use this installation's runtime.
        let mut paths = vec![node.parent().ok_or(Error("harness_node"))?.to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        cmd.env(
            "PATH",
            std::env::join_paths(paths).map_err(|_| Error("harness_node"))?,
        );
        cmd.env_remove("NODE_OPTIONS").env_remove("NODE_PATH");
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("AWM_") || key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    #[cfg(windows)]
    let job = crate::git::windows::Job::new().map_err(|_| Error("harness_disconnect"))?;
    #[cfg(windows)]
    cmd.creation_flags(
        windows_sys::Win32::System::Threading::CREATE_SUSPENDED
            | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
    );
    let child = cmd.spawn().map_err(|_| Error("harness_disconnect"))?;
    #[cfg(windows)]
    job.assign_and_resume(&child)
        .map_err(|_| Error("harness_disconnect"))?;
    Ok(Owned {
        child: Some(child),
        #[cfg(windows)]
        job: Some(job),
    })
}
pub async fn version(path: &Path) -> Result<String, Error> {
    version_output(
        spawn(path, &["--version"], &std::env::temp_dir(), None)?,
        Duration::from_secs(3),
    )
    .await
}
fn spawn_npm(npm: &npm::Launch, args: &[&str], cwd: &Path) -> Result<Owned, Error> {
    npm::validate(npm)?;
    let mut argv = vec![npm.entry.to_str().ok_or(Error("harness_path"))?];
    argv.extend_from_slice(args);
    spawn(&npm.node, &argv, cwd, Some(&npm.node))
}
pub async fn npm_version(npm: &npm::Launch) -> Result<String, Error> {
    let candidate = npm::validate(npm)?;
    let node = version_output(
        spawn(
            &npm.node,
            &["--version"],
            &std::env::temp_dir(),
            Some(&npm.node),
        )?,
        Duration::from_secs(3),
    )
    .await?;
    if node
        .split('.')
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        < candidate.node_major_min
    {
        return Err(Error("harness_node"));
    }
    let actual = version_output(
        spawn_npm(npm, &["--version"], &std::env::temp_dir())?,
        Duration::from_secs(12),
    )
    .await?;
    if actual != candidate.version {
        return Err(Error("harness_package"));
    }
    Ok(actual)
}
async fn version_output(mut owned: Owned, timeout: Duration) -> Result<String, Error> {
    let child = owned.child.as_mut().unwrap();
    child.stdin.take();
    let mut out = child.stdout.take().unwrap().take(1025);
    let mut bytes = vec![];
    tokio::time::timeout(timeout, async {
        out.read_to_end(&mut bytes)
            .await
            .map_err(|_| Error("harness_disconnect"))?;
        if !child
            .wait()
            .await
            .map_err(|_| Error("harness_disconnect"))?
            .success()
        {
            return Err(Error("harness_incompatible"));
        }
        Ok::<_, Error>(())
    })
    .await
    .map_err(|_| Error("harness_timeout"))??;
    if bytes.len() > 1024 {
        return Err(Error("harness_limit"));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Error("harness_protocol"))?;
    text.split_whitespace()
        .map(|s| s.trim_start_matches('v'))
        .find(|s| {
            s.len() < 48
                && s.split('.').count() == 3
                && s.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(str::to_owned)
        .ok_or(Error("harness_incompatible"))
}
async fn frame(reader: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> Result<Value, Error> {
    let mut bytes = vec![];
    loop {
        let part = reader
            .fill_buf()
            .await
            .map_err(|_| Error("harness_disconnect"))?;
        if part.is_empty() {
            return Err(Error("harness_disconnect"));
        }
        let n = part
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(part.len());
        if bytes.len() + n > FRAME {
            return Err(Error("harness_limit"));
        }
        let done = part[n - 1] == b'\n';
        bytes.extend_from_slice(&part[..n]);
        reader.consume(n);
        if done {
            break;
        }
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| Error("harness_protocol"))?;
    if !v.is_object() || v["jsonrpc"] != "2.0" {
        return Err(Error("harness_protocol"));
    }
    Ok(v)
}
async fn write(stdin: &mut ChildStdin, v: Value) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(&v).map_err(|_| Error("harness_protocol"))?;
    if bytes.len() >= FRAME {
        return Err(Error("harness_limit"));
    }
    bytes.push(b'\n');
    tokio::time::timeout(Duration::from_secs(2), stdin.write_all(&bytes))
        .await
        .map_err(|_| Error("harness_timeout"))?
        .map_err(|_| Error("harness_disconnect"))
}
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(str::to_owned)
}
fn id(v: &Value) -> Option<String> {
    text(v).filter(|s| s.len() <= 256)
}
pub(crate) fn event(s: &mut Instance, session: Option<String>, kind: &str, value: String) {
    s.cursor += 1;
    if s.events.len() >= EVENTS {
        s.events.pop_front();
    }
    s.events.push_back(Event {
        sequence: s.cursor,
        session_id: session,
        kind: kind.into(),
        text: value,
    });
}
enum Pending {
    Init,
    New,
    Prompt(String),
    Mode(String, String),
    Config(String),
    History,
    Load(String),
}
fn pending_conflict(action: &Action, pending: &Pending) -> bool {
    match (action, pending) {
        (Action::Prompt { session_id, .. }, Pending::Prompt(id) | Pending::Config(id))
        | (Action::Config { session_id, .. }, Pending::Prompt(id) | Pending::Config(id))
        | (Action::Config { session_id, .. }, Pending::Mode(id, _) | Pending::Load(id))
        | (
            Action::Mode { session_id, .. } | Action::Load { session_id, .. },
            Pending::Config(id),
        ) => session_id == id,
        _ => false,
    }
}
async fn run(mut owned: Owned, state: Arc<Mutex<Instance>>, mut rx: mpsc::Receiver<Action>) {
    let result = connection(&mut owned, &state, &mut rx).await;
    #[cfg(windows)]
    drop(owned.job.take());
    if let Some(mut child) = owned.child.take() {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        #[cfg(windows)]
        let _ = child.start_kill();
        if tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
    let mut s = state.lock().unwrap();
    s.connection = if result.is_ok() {
        "stopped"
    } else {
        "disconnected"
    }
    .into();
    s.stale = true;
    s.permissions.clear();
    for session in &mut s.sessions {
        session.activity = Activity::Unknown;
    }
    if let Err(e) = result {
        s.error = Some(e.0.into());
    }
}
async fn connection(
    owned: &mut Owned,
    state: &Arc<Mutex<Instance>>,
    rx: &mut mpsc::Receiver<Action>,
) -> Result<(), Error> {
    let child = owned.child.as_mut().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write(&mut stdin,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"branchline","version":env!("CARGO_PKG_VERSION")}}})).await?;
    let mut next = 2u64;
    let mut pending = HashMap::from([(1, (Instant::now(), Pending::Init))]);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // Keep partial framing across select cancellation; frame reads in its own bounded channel task.
    let (frames, mut incoming) = mpsc::channel(8);
    let reader = tokio::spawn(async move {
        loop {
            let v = frame(&mut stdout).await;
            let end = v.is_err();
            if frames.send(v).await.is_err() || end {
                break;
            }
        }
    });
    struct Reader(tokio::task::JoinHandle<()>);
    impl Drop for Reader {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader = Reader(reader);
    loop {
        tokio::select! {
            _=tick.tick()=>{if pending.values().any(|(at,p)|at.elapsed()>Duration::from_secs(if matches!(p,Pending::Prompt(_)){300}else{12})){return Err(Error("harness_timeout"));}},
            input=rx.recv()=>{
                let Some(input)=input else{return Ok(());};
                if matches!(input,Action::Stop{..}){return Ok(());}
                if let Action::Permission{permission_id,option_id,..}=&input {
                    let reply={let mut s=state.lock().unwrap();permission_reply(&mut s,*permission_id,option_id.as_deref())};
                    match reply{Ok(v)=>write(&mut stdin,v).await?,Err(e)=>{let mut s=state.lock().unwrap();event(&mut s,None,"error",e.0.into());}}continue;
                }
                if let Action::Cancel{session_id,..}=&input{
                    let replies={let mut s=state.lock().unwrap();let ids:Vec<_>=s.permissions.iter().filter(|p|&p.session_id==session_id).map(|p|p.id).collect();ids.into_iter().filter_map(|id|permission_reply(&mut s,id,None).ok()).collect::<Vec<_>>()};
                    for v in replies{write(&mut stdin,v).await?;}
                }
                if (!matches!(input,Action::Cancel{..}) && pending.len()>=8) || pending.values().any(|(_,p)|pending_conflict(&input,p)){let mut s=state.lock().unwrap();event(&mut s,None,"error","harness_busy".into());continue;}
                let prepared={let mut s=state.lock().unwrap();prepare(&mut s,input,next)};
                match prepared{Ok(Some((method,params,p)))=>{let mut v=json!({"jsonrpc":"2.0","method":method,"params":params});if let Some(p)=p{v["id"]=json!(next);pending.insert(next,(Instant::now(),p));next+=1;}write(&mut stdin,v).await?;},Ok(None)=>{},Err(e)=>{let mut s=state.lock().unwrap();event(&mut s,None,"error",e.0.into());}}
            },
            value=incoming.recv()=>{
                let v=value.ok_or(Error("harness_disconnect"))??;
                if v.get("method").is_some(){
                    let active=pending.values().any(|(_,p)|matches!(p,Pending::Prompt(id) if v["params"]["sessionId"].as_str()==Some(id.as_str())));
                    let reply={let mut s=state.lock().unwrap();notification(&mut s,&v,active)?};if let Some(reply)=reply{write(&mut stdin,reply).await?;}
                }else{
                    let rid=v["id"].as_u64().ok_or(Error("harness_protocol"))?;
                    let Some((_,p))=pending.remove(&rid) else{return Err(Error("harness_protocol"));};
                    let mut s=state.lock().unwrap();
                    if v.get("error").is_some(){if let Pending::Prompt(ref id)=p{if let Some(t)=s.sessions.iter_mut().find(|t|&t.id==id){t.activity=Activity::Error;}event(&mut s,Some(id.clone()),"attention_error",String::new());} event(&mut s,None,"error","ACP request rejected; raw provider error withheld".into());if matches!(p,Pending::Init){return Err(Error("harness_incompatible"));}}
                    else{response(&mut s,p,v.get("result").ok_or(Error("harness_protocol"))?)?;}
                }
            }
        }
    }
}
type Prepared = Option<(&'static str, Value, Option<Pending>)>;
fn prepare(s: &mut Instance, a: Action, _next: u64) -> Result<Prepared, Error> {
    if s.connection != "connected" {
        return Err(Error("harness_busy"));
    }
    match a {
        Action::New { .. } => {
            if s.sessions.len() >= 64 {
                return Err(Error("harness_limit"));
            }
            Ok(Some((
                "session/new",
                json!({"cwd":s.directory,"mcpServers":[]}),
                Some(Pending::New),
            )))
        }
        Action::History { .. } => {
            if !s.capabilities.list {
                return Err(Error("harness_incompatible"));
            }
            Ok(Some((
                "session/list",
                json!({"cwd":s.directory}),
                Some(Pending::History),
            )))
        }
        Action::Load { session_id, .. } => {
            if !s.capabilities.load || !s.sessions.iter().any(|t| t.id == session_id && t.assigned)
            {
                return Err(Error("harness_scope"));
            }
            Ok(Some((
                "session/load",
                json!({"sessionId":session_id,"cwd":s.directory,"mcpServers":[]}),
                Some(Pending::Load(session_id)),
            )))
        }
        Action::Prompt {
            session_id,
            text: prompt,
            ..
        } => {
            if prompt.is_empty() || prompt.len() > 8192 {
                return Err(Error("harness_limit"));
            }
            let t = s
                .sessions
                .iter_mut()
                .find(|t| t.id == session_id && t.assigned)
                .ok_or(Error("harness_scope"))?;
            if matches!(t.activity, Activity::Working | Activity::Waiting) {
                return Err(Error("harness_busy"));
            }
            t.activity = Activity::Unknown;
            Ok(Some((
                "session/prompt",
                json!({"sessionId":session_id,"prompt":[{"type":"text","text":prompt}]}),
                Some(Pending::Prompt(session_id)),
            )))
        }
        Action::Cancel { session_id, .. } => {
            if !s.sessions.iter().any(|t| t.id == session_id && t.assigned) {
                return Err(Error("harness_scope"));
            }
            Ok(Some((
                "session/cancel",
                json!({"sessionId":session_id}),
                None,
            )))
        }
        Action::Mode {
            session_id,
            mode_id,
            ..
        } => {
            let t = s
                .sessions
                .iter()
                .find(|t| t.id == session_id && t.assigned)
                .ok_or(Error("harness_scope"))?;
            if !t.modes.iter().any(|m| m.id == mode_id) {
                return Err(Error("harness_incompatible"));
            }
            Ok(Some((
                "session/set_mode",
                json!({"sessionId":session_id,"modeId":mode_id}),
                Some(Pending::Mode(session_id, mode_id)),
            )))
        }
        Action::Config {
            session_id,
            config_id,
            value,
            ..
        } => {
            let t = s
                .sessions
                .iter()
                .find(|t| t.id == session_id && t.assigned)
                .ok_or(Error("harness_scope"))?;
            if matches!(t.activity, Activity::Working | Activity::Waiting) {
                return Err(Error("harness_busy"));
            }
            if !t
                .config_options
                .iter()
                .any(|c| c.id == config_id && c.options.iter().any(|o| o.id == value))
            {
                return Err(Error("harness_incompatible"));
            }
            Ok(Some((
                "session/set_config_option",
                json!({"sessionId":session_id,"configId":config_id,"value":value}),
                Some(Pending::Config(session_id)),
            )))
        }
        // Permission replies are handled separately by the actor, never sent as requests.
        _ => Err(Error("harness_permission")),
    }
}
fn permission_reply(s: &mut Instance, pid: u64, option: Option<&str>) -> Result<Value, Error> {
    let n = s
        .permissions
        .iter()
        .position(|p| p.id == pid)
        .ok_or(Error("harness_permission"))?;
    let p = &s.permissions[n];
    if option.is_some_and(|id| !p.options.iter().any(|o| o.id == id)) {
        return Err(Error("harness_permission"));
    }
    let p = s.permissions.remove(n);
    if let Some(t) = s.sessions.iter_mut().find(|t| t.id == p.session_id) {
        t.activity = Activity::Unknown;
    }
    Ok(
        json!({"jsonrpc":"2.0","id":p.rpc_id,"result":{"outcome":match option{Some(id)=>json!({"outcome":"selected","optionId":id}),None=>json!({"outcome":"cancelled"})}}}),
    )
}
fn choices(v: &Value, id_key: &str) -> Vec<Choice> {
    v.as_array()
        .map(|a| {
            a.iter()
                .take(32)
                .filter_map(|m| {
                    Some(Choice {
                        id: id(&m[id_key])?,
                        name: text(&m["name"])?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
fn config_options(v: &Value) -> Result<Vec<ConfigOption>, Error> {
    let values = v.as_array().ok_or(Error("harness_protocol"))?;
    if values.len() > 32 {
        return Err(Error("harness_limit"));
    }
    let mut result = Vec::new();
    for v in values {
        // Unknown extension types are not editable through the select-only client.
        if v["type"] != "select" {
            continue;
        }
        let config_id = id(&v["id"]).ok_or(Error("harness_protocol"))?;
        if result.iter().any(|c: &ConfigOption| c.id == config_id) {
            return Err(Error("harness_protocol"));
        }
        let mut options = Vec::new();
        let items = v["options"].as_array().ok_or(Error("harness_protocol"))?;
        if items.len() > 128 {
            return Err(Error("harness_limit"));
        }
        for item in items {
            let entries = if item.get("group").is_some() {
                item["options"]
                    .as_array()
                    .ok_or(Error("harness_protocol"))?
                    .as_slice()
            } else {
                std::slice::from_ref(item)
            };
            if options.len() + entries.len() > 128 {
                return Err(Error("harness_limit"));
            }
            for entry in entries {
                let value = id(&entry["value"]).ok_or(Error("harness_protocol"))?;
                if options.iter().any(|o: &Choice| o.id == value) {
                    return Err(Error("harness_protocol"));
                }
                options.push(Choice {
                    id: value,
                    name: text(&entry["name"]).ok_or(Error("harness_protocol"))?,
                });
            }
        }
        result.push(ConfigOption {
            id: config_id,
            name: text(&v["name"]).ok_or(Error("harness_protocol"))?,
            current_value: id(&v["currentValue"]).ok_or(Error("harness_protocol"))?,
            options,
        });
    }
    Ok(result)
}
fn metadata(t: &mut Session, v: &Value) -> Result<(), Error> {
    // Parse before mutation: malformed config cannot partially replace valid controls.
    let config = v.get("configOptions").map(config_options).transpose()?;
    if v.get("modes").is_some() {
        t.modes = choices(&v["modes"]["availableModes"], "id");
        t.current_mode = id(&v["modes"]["currentModeId"]);
    }
    if let Some(config) = config {
        t.config_options = config;
    }
    Ok(())
}
fn response(s: &mut Instance, p: Pending, v: &Value) -> Result<(), Error> {
    match p {
        Pending::Init => {
            if v["protocolVersion"] != 1 || !v["agentCapabilities"].is_object() {
                return Err(Error("harness_incompatible"));
            }
            let c = &v["agentCapabilities"];
            s.capabilities = Capabilities {
                load: c["loadSession"] == true,
                list: c["sessionCapabilities"]["list"].is_object(),
                image: c["promptCapabilities"]["image"] == true,
                audio: c["promptCapabilities"]["audio"] == true,
                embedded_context: c["promptCapabilities"]["embeddedContext"] == true,
            };
            s.protocol_version = Some(1);
            s.connection = "connected".into();
            s.stale = false;
        }
        Pending::New => {
            let sid = id(&v["sessionId"]).ok_or(Error("harness_protocol"))?;
            if s.sessions.len() >= 64 {
                return Err(Error("harness_limit"));
            }
            if let Some(t) = s.sessions.iter_mut().find(|t| t.id == sid) {
                if t.assigned {
                    return Err(Error("harness_protocol"));
                }
                t.assigned = true;
                t.directory = Some(s.directory.clone());
                metadata(t, v)?;
                return Ok(());
            }
            let mut t = Session {
                id: sid,
                parent_id: None,
                directory: Some(s.directory.clone()),
                assigned: true,
                activity: Activity::Unknown,
                modes: vec![],
                current_mode: None,
                config_options: vec![],
                commands: vec![],
            };
            metadata(&mut t, v)?;
            s.sessions.push(t);
        }
        Pending::Prompt(sid) => {
            let t = s
                .sessions
                .iter_mut()
                .find(|t| t.id == sid)
                .ok_or(Error("harness_scope"))?;
            t.activity = if v["stopReason"].is_string() {
                Activity::Idle
            } else {
                return Err(Error("harness_protocol"));
            };
            event(
                s,
                Some(sid),
                "turn_end",
                text(&v["stopReason"]).unwrap_or_default(),
            );
        }
        Pending::Mode(sid, mode) => {
            let t = s
                .sessions
                .iter_mut()
                .find(|t| t.id == sid)
                .ok_or(Error("harness_scope"))?;
            t.current_mode = Some(mode);
        }
        Pending::Config(sid) => {
            let config = config_options(v.get("configOptions").ok_or(Error("harness_protocol"))?)?;
            let t = s
                .sessions
                .iter_mut()
                .find(|t| t.id == sid && t.assigned)
                .ok_or(Error("harness_scope"))?;
            t.config_options = config;
        }
        Pending::Load(sid) => {
            if let Some(t) = s.sessions.iter_mut().find(|t| t.id == sid) {
                metadata(t, v)?;
                t.activity = Activity::Unknown;
            }
        }
        Pending::History => {
            let sessions = v["sessions"].as_array().ok_or(Error("harness_protocol"))?;
            if sessions.len() > 64 {
                return Err(Error("harness_limit"));
            }
            for v in sessions {
                let sid = id(&v["sessionId"]).ok_or(Error("harness_protocol"))?;
                if s.sessions.iter().any(|t| t.id == sid) {
                    continue;
                }
                if s.sessions.len() >= 64 {
                    return Err(Error("harness_limit"));
                }
                let cwd = v["cwd"]
                    .as_str()
                    .and_then(|p| crate::environment::canonical_path(Path::new(p)).ok());
                let assigned = cwd.as_ref() == Some(&s.directory);
                s.sessions.push(Session {
                    id: sid,
                    parent_id: None,
                    directory: cwd,
                    assigned,
                    activity: Activity::Unknown,
                    modes: vec![],
                    current_mode: None,
                    config_options: vec![],
                    commands: vec![],
                });
            }
        }
    }
    Ok(())
}
fn notification(s: &mut Instance, v: &Value, active: bool) -> Result<Option<Value>, Error> {
    let p = &v["params"];
    let method = v["method"].as_str().ok_or(Error("harness_protocol"))?;
    if method == "session/request_permission" {
        let rid = v["id"].clone();
        if !(rid.is_u64() || id(&rid).is_some()) {
            return Err(Error("harness_protocol"));
        }
        let sid = id(&p["sessionId"]).ok_or(Error("harness_protocol"))?;
        let Some(t) = s
            .sessions
            .iter_mut()
            .find(|t| t.id == sid && t.assigned && active)
        else {
            return Ok(Some(
                json!({"jsonrpc":"2.0","id":rid,"result":{"outcome":{"outcome":"cancelled"}}}),
            ));
        };
        if s.permissions.len() >= 8 {
            return Err(Error("harness_limit"));
        }
        t.activity = Activity::Waiting;
        s.permissions.push(Permission {
            rpc_id: rid,
            id: s.cursor + 1,
            session_id: sid.clone(),
            title: text(&p["toolCall"]["title"]).unwrap_or_else(|| "Tool permission".into()),
            options: choices(&p["options"], "optionId"),
        });
        s.cursor += 1;
        event(s, Some(sid), "attention_waiting", String::new());
        return Ok(None);
    }
    if v.get("id").is_some() {
        return Ok(Some(
            json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"Client capability not supported"}}),
        ));
    }
    if method != "session/update" {
        return Ok(None);
    }
    let sid = id(&p["sessionId"]).ok_or(Error("harness_protocol"))?;
    let u = &p["update"];
    let kind = u["sessionUpdate"]
        .as_str()
        .ok_or(Error("harness_protocol"))?;
    if kind.len() > 64
        || u["content"]["text"]
            .as_str()
            .is_some_and(|s| s.len() > 4096)
    {
        return Err(Error("harness_limit"));
    }
    if !s.sessions.iter().any(|t| t.id == sid) {
        if s.sessions.len() >= 64 {
            return Err(Error("harness_limit"));
        }
        s.sessions.push(Session {
            id: sid.clone(),
            parent_id: None,
            directory: None,
            assigned: false,
            activity: Activity::Unknown,
            modes: vec![],
            current_mode: None,
            config_options: vec![],
            commands: vec![],
        });
    }
    let t = s.sessions.iter_mut().find(|t| t.id == sid).unwrap();
    if t.assigned {
        match kind {
            "config_option_update" => {
                t.config_options =
                    config_options(u.get("configOptions").ok_or(Error("harness_protocol"))?)?;
            }
            "current_mode_update" => {
                t.current_mode = id(&u["currentModeId"]);
            }
            "available_commands_update" => {
                let commands = u["availableCommands"]
                    .as_array()
                    .ok_or(Error("harness_protocol"))?;
                if commands.len() > 64 {
                    return Err(Error("harness_limit"));
                }
                t.commands = commands
                    .iter()
                    .filter_map(|c| {
                        let name = id(&c["name"])?;
                        // Commands only fill the composer; no extension RPC or automatic execution.
                        if !name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c))
                        {
                            return None;
                        }
                        Some(AvailableCommand {
                            name,
                            description: text(&c["description"]).unwrap_or_default(),
                        })
                    })
                    .collect();
            }
            _ => {}
        }
    }
    if t.assigned
        && active
        && matches!(
            kind,
            "agent_message_chunk"
                | "agent_thought_chunk"
                | "tool_call"
                | "tool_call_update"
                | "plan"
        )
    {
        t.activity = Activity::Working;
    }
    let value = if matches!(kind, "agent_message_chunk" | "user_message_chunk") {
        text(&u["content"]["text"])
    } else if matches!(kind, "tool_call" | "tool_call_update") {
        text(&u["title"])
    } else {
        None
    };
    if let Some(value) = value {
        event(s, Some(sid), kind, value);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn instance() -> Instance {
        Instance {
            id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            installation_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            directory: std::env::temp_dir(),
            connection: "connected".into(),
            stale: false,
            error: None,
            owned: true,
            protocol_version: Some(1),
            capabilities: Capabilities::default(),
            sessions: vec![],
            permissions: vec![],
            events: VecDeque::new(),
            cursor: 0,
        }
    }
    #[test]
    fn replay_unassigned_and_live_updates_do_not_invent_activity() {
        let mut s = instance();
        response(&mut s, Pending::New, &json!({"sessionId":"a"})).unwrap();
        let update = |id| json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":id,"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"chunk"}}}});
        notification(&mut s, &update("a"), false).unwrap();
        assert_eq!(s.sessions[0].activity, Activity::Unknown);
        notification(&mut s, &update("foreign-child"), true).unwrap();
        assert!(!s.sessions[1].assigned);
        assert_eq!(s.sessions[1].activity, Activity::Unknown);
        notification(&mut s, &update("a"), true).unwrap();
        assert_eq!(s.sessions[0].activity, Activity::Working);
        response(
            &mut s,
            Pending::Prompt("a".into()),
            &json!({"stopReason":"end_turn"}),
        )
        .unwrap();
        assert_eq!(s.sessions[0].activity, Activity::Idle);
        for _ in 0..EVENTS + 10 {
            notification(&mut s, &update("a"), true).unwrap();
        }
        assert_eq!(s.events.len(), EVENTS);
        assert!(s.events[0].sequence > 1);
    }
    #[test]
    fn permission_is_exact_session_and_option_scoped_and_preserves_rpc_id() {
        let mut s = instance();
        response(&mut s, Pending::New, &json!({"sessionId":"a"})).unwrap();
        let v = json!({"jsonrpc":"2.0","id":"rpc-string","method":"session/request_permission","params":{"sessionId":"a","toolCall":{"title":"tool"},"options":[{"optionId":"yes","name":"Allow"}]}});
        assert!(notification(&mut s, &v, false).unwrap().is_some());
        assert!(s.permissions.is_empty());
        notification(&mut s, &v, true).unwrap();
        assert_eq!(s.sessions[0].activity, Activity::Waiting);
        let id = s.permissions[0].id;
        assert!(permission_reply(&mut s, id, Some("other")).is_err());
        assert_eq!(s.permissions.len(), 1);
        let reply = permission_reply(&mut s, id, None).unwrap();
        assert_eq!(reply["id"], "rpc-string");
        assert_eq!(reply["result"]["outcome"]["outcome"], "cancelled");
        assert_eq!(s.sessions[0].activity, Activity::Unknown);
        assert!(permission_reply(&mut s, id, None).is_err());
    }
    #[test]
    fn negotiation_and_controls_fail_closed() {
        let mut s = instance();
        assert!(response(
            &mut s,
            Pending::Init,
            &json!({"protocolVersion":2,"agentCapabilities":{}})
        )
        .is_err());
        response(
            &mut s,
            Pending::Init,
            &json!({"protocolVersion":1,"agentCapabilities":{}}),
        )
        .unwrap();
        assert!(!s.capabilities.load && !s.capabilities.list);
        assert!(prepare(
            &mut s,
            Action::History {
                instance_id: Uuid::nil()
            },
            2
        )
        .is_err());
        response(&mut s, Pending::New, &json!({"sessionId":"a"})).unwrap();
        assert_eq!(s.sessions[0].activity, Activity::Unknown);
        assert!(prepare(
            &mut s,
            Action::Mode {
                instance_id: Uuid::nil(),
                session_id: "a".into(),
                mode_id: "unchecked".into()
            },
            2
        )
        .is_err());
    }
    #[tokio::test]
    async fn framing_rejects_overflow_invalid_and_accepts_crlf() {
        let v = frame(&mut BufReader::new(
            &b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n"[..],
        ))
        .await
        .unwrap();
        assert_eq!(v["id"], 1);
        assert!(frame(&mut BufReader::new(&b"{}\n"[..])).await.is_err());
        assert_eq!(
            frame(&mut BufReader::new(&vec![b'x'; FRAME + 1][..]))
                .await
                .unwrap_err()
                .0,
            "harness_limit"
        );
    }
    #[test]
    fn missing_capabilities_are_not_enabled() {
        let c: Capabilities = Default::default();
        assert!(!c.load && !c.list && !c.image);
    }
    fn config(value: &str) -> Value {
        json!([{"id":"reasoning","name":"Reasoning","type":"select","currentValue":value,
            "options":[{"group":"levels","name":"Levels","options":[
                {"value":"low","name":"Low"},{"value":"high","name":"High"}]}]}])
    }
    #[test]
    fn config_is_scoped_to_confirmed_metadata_and_waits_for_authoritative_response() {
        let mut s = instance();
        response(
            &mut s,
            Pending::New,
            &json!({"sessionId":"a","configOptions":config("low")}),
        )
        .unwrap();
        let action = |sid: &str, value: &str| Action::Config {
            instance_id: Uuid::nil(),
            session_id: sid.into(),
            config_id: "reasoning".into(),
            value: value.into(),
        };
        assert!(prepare(&mut s, action("foreign", "high"), 2).is_err());
        assert!(prepare(&mut s, action("a", "forged"), 2).is_err());
        let (method, params, _) = prepare(&mut s, action("a", "high"), 2).unwrap().unwrap();
        assert_eq!(method, "session/set_config_option");
        assert_eq!(
            params,
            json!({"sessionId":"a","configId":"reasoning","value":"high"})
        );
        assert_eq!(s.sessions[0].config_options[0].current_value, "low");
        response(
            &mut s,
            Pending::Config("a".into()),
            &json!({"configOptions":config("high")}),
        )
        .unwrap();
        assert_eq!(s.sessions[0].config_options[0].current_value, "high");
        s.sessions[0].activity = Activity::Waiting;
        assert_eq!(
            prepare(&mut s, action("a", "low"), 2).err().unwrap().0,
            "harness_busy"
        );
    }
    #[test]
    fn config_and_command_updates_are_bounded_and_do_not_imply_activity() {
        let mut s = instance();
        response(
            &mut s,
            Pending::New,
            &json!({"sessionId":"a","configOptions":config("low")}),
        )
        .unwrap();
        let update = |kind: &str, key: &str, value: Value| {
            let mut u = json!({"sessionUpdate":kind});
            u[key] = value;
            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"a","update":u}})
        };
        notification(
            &mut s,
            &update("config_option_update", "configOptions", json!([])),
            false,
        )
        .unwrap();
        assert!(s.sessions[0].config_options.is_empty());
        notification(&mut s, &update("available_commands_update", "availableCommands", json!([
            {"name":"mcp","description":"Show MCP status"},{"name":"bad\ncommand","description":""}]
        )), false).unwrap();
        assert_eq!(s.sessions[0].commands.len(), 1);
        assert_eq!(s.sessions[0].activity, Activity::Unknown);
        let mut invalid = config("high");
        invalid[0]["options"] =
            json!([{"value":"high","name":"High"},{"value":"high","name":"Duplicate"}]);
        assert!(config_options(&invalid).is_err());
        assert_eq!(
            config_options(&json!(vec![config("low")[0].clone(); 33]))
                .unwrap_err()
                .0,
            "harness_limit"
        );
        assert!(notification(
            &mut s,
            &json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"a","update":{"sessionUpdate":"config_option_update"}}}),
            false
        )
        .is_err());
    }
    #[test]
    fn pending_config_is_serialized_with_turns_and_session_controls() {
        let action = Action::Config {
            instance_id: Uuid::nil(),
            session_id: "a".into(),
            config_id: "reasoning".into(),
            value: "high".into(),
        };
        assert!(pending_conflict(&action, &Pending::Prompt("a".into())));
        assert!(pending_conflict(&action, &Pending::Config("a".into())));
        assert!(!pending_conflict(&action, &Pending::Config("other".into())));
        assert!(pending_conflict(
            &Action::Prompt {
                instance_id: Uuid::nil(),
                session_id: "a".into(),
                text: "prompt".into(),
            },
            &Pending::Config("a".into())
        ));
        assert!(!pending_conflict(
            &Action::Cancel {
                instance_id: Uuid::nil(),
                session_id: "a".into(),
            },
            &Pending::Config("a".into())
        ));
    }
}
