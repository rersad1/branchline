//! Bounded environment-local OpenCode adapter (D5.1) and external HTTP/SSE observer (D5.3).
use crate::{environment::canonical_path, service::WorkspaceService};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const OPENCODE_VERSION: &str = "1.19.0";
const BODY: usize = 2 * 1024 * 1024;
const FRAME: usize = 64 * 1024;
const SESSIONS: usize = 512;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub request_id: Uuid,
    /// Only an IPv4 loopback port in the executing environment; never a URL/proxy.
    pub port: u16,
    pub cancel: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Response {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub request_id: Uuid,
    pub port: u16,
    pub observation: Option<Observation>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Observation {
    pub version: String,
    pub directory: PathBuf,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub sessions: Vec<Session>,
    pub sse_connected: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub status: Status,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum Status {
    Unknown,
    Working,
    Idle,
}
#[derive(Debug, thiserror::Error)]
#[error("{}", message(.0))]
pub struct Error(pub &'static str);
pub fn message(code: &str) -> &'static str {
    match code {
        "provider_scope" => {
            "Provider directory/worktree identity is not confirmed in this environment."
        }
        "provider_auth" => {
            "OpenCode authorization failed; configure credentials in the provider environment."
        }
        "provider_incompatible" => "OpenCode API is incompatible; this adapter requires 1.19.0.",
        "provider_malformed" => "OpenCode returned a malformed response.",
        "provider_limit" => "Provider response, frame, session or concurrency limit exceeded.",
        "provider_timeout" => "Provider contract check timed out.",
        "provider_cancelled" => "Provider contract check cancelled.",
        "provider_disconnect" => "OpenCode SSE disconnected before confirmation.",
        _ => "OpenCode is unavailable in the selected environment.",
    }
}
type Key = (Uuid, Uuid, Uuid, u16);
#[derive(Clone, Default)]
pub struct ProviderService {
    active: Arc<Mutex<Requests>>,
}
#[derive(Default)]
struct Requests {
    running: HashMap<Key, Option<tokio::sync::oneshot::Sender<()>>>,
    cancelled: HashMap<Key, std::time::Instant>,
}
struct Guard {
    service: ProviderService,
    key: Key,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.service
            .active
            .lock()
            .expect("provider requests")
            .running
            .remove(&self.key);
    }
}
impl ProviderService {
    pub async fn execute(&self, service: &WorkspaceService, r: Request) -> Result<Response, Error> {
        if r.port == 0 || r.request_id.is_nil() {
            return Err(Error("provider_scope"));
        }
        let key = (r.project_id, r.worktree_id, r.request_id, r.port);
        let response = |observation| Response {
            environment_id: service.environment.id,
            project_id: r.project_id,
            worktree_id: r.worktree_id,
            request_id: r.request_id,
            port: r.port,
            observation,
        };
        // Cancellation bypasses concurrency limits and never stores an unbounded tombstone queue.
        if r.cancel {
            let mut state = self.active.lock().expect("provider requests");
            if let Some(sender) = state.running.get_mut(&key) {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            } else {
                // IPC scheduling can deliver cancel before inspect. Bounded, expiring markers.
                state
                    .cancelled
                    .retain(|_, at| at.elapsed() < Duration::from_secs(20));
                if state.cancelled.len() >= 64 {
                    return Err(Error("provider_limit"));
                }
                state.cancelled.insert(key, std::time::Instant::now());
            }
            return Ok(response(None));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut active = self.active.lock().expect("provider requests");
            active
                .cancelled
                .retain(|_, at| at.elapsed() < Duration::from_secs(20));
            if active.cancelled.remove(&key).is_some() {
                return Err(Error("provider_cancelled"));
            }
            if active.running.len() >= 4 || active.running.contains_key(&key) {
                return Err(Error("provider_limit"));
            }
            active.running.insert(key, Some(sender));
        }
        let _guard = Guard {
            service: self.clone(),
            key,
        };
        tokio::select! {
            _ = receiver => Err(Error("provider_cancelled")),
            result = tokio::time::timeout(Duration::from_secs(12), inspect(service, &r)) => {
                result.map_err(|_| Error("provider_timeout"))?.map(|o| response(Some(o)))
            }
        }
    }
}
async fn directory(service: &WorkspaceService, r: &Request) -> Result<PathBuf, Error> {
    // Existing discovery validates canonical root/common-dir/admin identity, including external trees.
    let snapshot = service
        .worktrees
        .refresh(r.project_id)
        .await
        .map_err(|_| Error("provider_scope"))?;
    let tree = snapshot
        .worktrees
        .iter()
        .find(|t| t.id == r.worktree_id)
        .ok_or(Error("provider_scope"))?;
    if snapshot.environment_id != service.environment.id
        || snapshot.error.is_some()
        || !tree.is_available
        || tree.is_bare
        || tree.availability_error.is_some()
    {
        return Err(Error("provider_scope"));
    }
    Ok(tree.path.clone())
}
fn canonical(value: &str) -> Result<PathBuf, Error> {
    if !crate::environment::valid_path(value) {
        return Err(Error("provider_scope"));
    }
    canonical_path(Path::new(value)).map_err(|_| Error("provider_scope"))
}
struct OpenCode {
    client: reqwest::Client,
    base: String,
}
impl OpenCode {
    fn new(port: u16) -> Result<Self, Error> {
        let mut headers = reqwest::header::HeaderMap::new();
        // Provider-owned runtime configuration only. Never accept/forward credentials via IPC/settings.
        if let Ok(password) = std::env::var("OPENCODE_SERVER_PASSWORD") {
            let username =
                std::env::var("OPENCODE_SERVER_USERNAME").unwrap_or_else(|_| "opencode".into());
            let request = reqwest::Client::new()
                .get("http://127.0.0.1")
                .basic_auth(username, Some(password))
                .build()
                .map_err(|_| Error("provider_auth"))?;
            let mut auth = request.headers()[reqwest::header::AUTHORIZATION].clone();
            auth.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, auth);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| Error("provider_unavailable"))?;
        Ok(Self {
            client,
            base: format!("http://127.0.0.1:{port}"),
        })
    }
    async fn response(
        &self,
        route: &str,
        dir: Option<&str>,
        mime: &str,
    ) -> Result<reqwest::Response, Error> {
        let mut request = self.client.get(format!("{}{route}", self.base));
        if let Some(dir) = dir {
            request = request.query(&[("directory", dir)]);
        }
        let response = request
            .send()
            .await
            .map_err(|_| Error("provider_unavailable"))?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(Error("provider_auth"));
        }
        if !response.status().is_success() {
            return Err(Error(
                if response.status().is_redirection() || response.status().as_u16() == 404 {
                    "provider_incompatible"
                } else {
                    "provider_unavailable"
                },
            ));
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some(mime)
        {
            return Err(Error("provider_malformed"));
        }
        Ok(response)
    }
    async fn json(&self, route: &str, dir: Option<&str>) -> Result<Value, Error> {
        let mut response = self.response(route, dir, "application/json").await?;
        if response.content_length().is_some_and(|n| n > BODY as u64) {
            return Err(Error("provider_limit"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error("provider_malformed"))?
        {
            if bytes.len() + chunk.len() > BODY {
                return Err(Error("provider_limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error("provider_malformed"))
    }
    async fn connected(&self, dir: &str) -> Result<(), Error> {
        let mut response = self
            .response("/event", Some(dir), "text/event-stream")
            .await?;
        let mut decoder = Decoder::default();
        loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|_| Error("provider_disconnect"))?
                .ok_or(Error("provider_disconnect"))?;
            for byte in chunk {
                if let Some(value) = decoder.byte(byte)? {
                    if value["type"] == "server.connected" {
                        return Ok(());
                    }
                    if value["type"].as_str().is_none() {
                        return Err(Error("provider_malformed"));
                    }
                }
            }
        }
    }
}
// No background task or event queue. Incremental byte decoder bounds even a huge HTTP chunk.
#[derive(Default)]
struct Decoder {
    frame: Vec<u8>,
}
impl Decoder {
    fn byte(&mut self, byte: u8) -> Result<Option<Value>, Error> {
        if self.frame.len() == FRAME {
            return Err(Error("provider_limit"));
        }
        self.frame.push(byte);
        if !(self.frame.ends_with(b"\n\n") || self.frame.ends_with(b"\r\n\r\n")) {
            return Ok(None);
        }
        let text = std::str::from_utf8(&self.frame).map_err(|_| Error("provider_malformed"))?;
        let data: Vec<_> = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|s| s.strip_prefix(' ').unwrap_or(s))
            })
            .collect();
        let value = if data.is_empty() {
            None
        } else {
            Some(serde_json::from_str(&data.join("\n")).map_err(|_| Error("provider_malformed"))?)
        };
        self.frame.clear();
        Ok(value)
    }
}
fn status(value: Option<&Value>) -> Result<Status, Error> {
    let Some(value) = value else {
        return Ok(Status::Unknown);
    };
    match value.get("type").and_then(Value::as_str) {
        Some("busy" | "retry") => Ok(Status::Working),
        Some("idle") => Ok(Status::Idle),
        Some(_) => Ok(Status::Unknown),
        None => Err(Error("provider_malformed")),
    }
}
async fn inspect(service: &WorkspaceService, r: &Request) -> Result<Observation, Error> {
    let dir = directory(service, r).await?;
    let dir_str = dir.to_str().ok_or(Error("provider_scope"))?;
    let adapter = OpenCode::new(r.port)?;
    let health = adapter.json("/global/health", None).await?;
    if health["version"] != OPENCODE_VERSION {
        return Err(Error("provider_incompatible"));
    }
    if health["healthy"] != true {
        return Err(Error("provider_unavailable"));
    }
    let spec = adapter.json("/doc", None).await?;
    for route in [
        "/global/health",
        "/path",
        "/session",
        "/session/status",
        "/event",
    ] {
        if !spec["paths"][route]["get"].is_object() {
            return Err(Error("provider_incompatible"));
        }
    }
    let path = adapter.json("/path", Some(dir_str)).await?;
    if canonical(
        path["directory"]
            .as_str()
            .ok_or(Error("provider_malformed"))?,
    )? != dir
    {
        return Err(Error("provider_scope"));
    }
    let list = adapter.json("/session", Some(dir_str)).await?;
    let list = list.as_array().ok_or(Error("provider_malformed"))?;
    if list.len() > SESSIONS {
        return Err(Error("provider_limit"));
    }
    let statuses = adapter.json("/session/status", Some(dir_str)).await?;
    let statuses = statuses.as_object().ok_or(Error("provider_malformed"))?;
    if statuses.len() > SESSIONS {
        return Err(Error("provider_limit"));
    }
    for value in statuses.values() {
        status(Some(value))?;
    }
    let mut sessions = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for session in list {
        let id = session["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
            .ok_or(Error("provider_malformed"))?;
        if !ids.insert(id) {
            return Err(Error("provider_malformed"));
        }
        if canonical(
            session["directory"]
                .as_str()
                .ok_or(Error("provider_malformed"))?,
        )? != dir
        {
            return Err(Error("provider_scope"));
        }
        sessions.push(Session {
            id: id.into(),
            parent_id: session["parentID"]
                .as_str()
                .filter(|id| id.len() <= 256)
                .map(str::to_owned),
            status: status(statuses.get(id))?,
        });
    }
    adapter.connected(dir_str).await?;
    // Revalidate after network I/O; replacement/unregistration must not yield a successful observation.
    if directory(service, r).await? != dir {
        return Err(Error("provider_scope"));
    }
    Ok(Observation {
        version: OPENCODE_VERSION.into(),
        directory: dir,
        observed_at: chrono::Utc::now(),
        sessions,
        sse_connected: true,
    })
}

/// External HTTP/SSE observation only. Stop/drop never terminates the external server.
pub(crate) async fn monitor(
    service: WorkspaceService,
    port: u16,
    state: Arc<Mutex<crate::acp::Instance>>,
    mut stop: tokio::sync::mpsc::Receiver<crate::acp::Action>,
) {
    let (project_id, worktree_id) = {
        let s = state.lock().unwrap();
        (s.project_id, s.worktree_id)
    };
    let request = Request {
        project_id,
        worktree_id,
        request_id: Uuid::new_v4(),
        port,
        cancel: false,
    };
    let mut failures = 0;
    loop {
        let result = tokio::select! {_ = stop.recv()=>break,r=tokio::time::timeout(Duration::from_secs(12),monitor_cycle(&service,&request,&state))=>r.unwrap_or(Err(Error("provider_timeout")))};
        if let Err(e) = result {
            failures += 1;
            {
                let mut s = state.lock().unwrap();
                s.connection = "disconnected".into();
                s.stale = true;
                s.error = Some(e.0.into());
                for t in &mut s.sessions {
                    t.activity = crate::acp::Activity::Unknown;
                }
            }
            if failures >= 3 {
                break;
            }
            tokio::select! {_=stop.recv()=>break,_=tokio::time::sleep(Duration::from_secs(1<<failures))=>{}}
        } else {
            failures = 0;
        }
    }
    let mut s = state.lock().unwrap();
    if s.error.is_none() {
        s.connection = "stopped".into();
    }
    s.stale = true;
    for t in &mut s.sessions {
        t.activity = crate::acp::Activity::Unknown;
    }
}
async fn monitor_cycle(
    service: &WorkspaceService,
    r: &Request,
    state: &Arc<Mutex<crate::acp::Instance>>,
) -> Result<(), Error> {
    use crate::acp::{Activity, Session as Mapped};
    // Resnapshot periodically and after every connection loss. No Last-Event-ID/replay assumption.
    {
        let mut s = state.lock().unwrap();
        s.events.clear();
        s.stale = true;
        for t in &mut s.sessions {
            t.activity = Activity::Unknown;
        }
    }
    let o = inspect(service, r).await?;
    if o.sessions.len() > 64 {
        return Err(Error("provider_limit"));
    }
    {
        let mut s = state.lock().unwrap();
        if o.directory != s.directory {
            return Err(Error("provider_scope"));
        }
        s.sessions = o
            .sessions
            .into_iter()
            .map(|t| Mapped {
                id: t.id,
                parent_id: t.parent_id,
                directory: Some(o.directory.clone()),
                assigned: true,
                activity: match t.status {
                    Status::Working => Activity::Working,
                    Status::Idle => Activity::Idle,
                    Status::Unknown => Activity::Unknown,
                },
                modes: vec![],
                current_mode: None,
                config_options: vec![],
                commands: vec![],
            })
            .collect();
        s.connection = "connected".into();
        s.error = None;
        s.stale = false;
    }
    let adapter = OpenCode::new(r.port)?;
    let mut response = adapter
        .response(
            "/event",
            Some(o.directory.to_str().ok_or(Error("provider_scope"))?),
            "text/event-stream",
        )
        .await?;
    let mut decoder = Decoder::default();
    let start = Instant::now();
    loop {
        let remaining = Duration::from_secs(5).saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Ok(());
        }
        let chunk = match tokio::time::timeout(remaining, response.chunk()).await {
            Err(_) => return Ok(()),
            Ok(r) => r
                .map_err(|_| Error("provider_disconnect"))?
                .ok_or(Error("provider_disconnect"))?,
        };
        for byte in chunk {
            if let Some(v) = decoder.byte(byte)? {
                let mut s = state.lock().unwrap();
                normalize_native(&mut s, &v)?;
            }
        }
    }
}
fn normalize_native(s: &mut crate::acp::Instance, v: &Value) -> Result<(), Error> {
    use crate::acp::Activity;
    let kind = v["type"].as_str().ok_or(Error("provider_malformed"))?;
    let p = &v["properties"];
    let Some(sid) = p["sessionID"].as_str() else {
        return Ok(());
    };
    let Some(t) = s.sessions.iter_mut().find(|t| t.id == sid && t.assigned) else {
        return Ok(());
    };
    t.activity = match kind {
        "session.status" => match status(Some(&p["status"]))? {
            Status::Working => Activity::Working,
            Status::Idle => Activity::Idle,
            Status::Unknown => Activity::Unknown,
        },
        "session.error" => Activity::Error,
        "permission.asked" => Activity::Waiting,
        "permission.replied" => Activity::Unknown,
        _ => return Ok(()),
    };
    if let Some(kind) = match kind {
        "permission.asked" => Some("attention_waiting"),
        "session.error" => Some("attention_error"),
        _ => None,
    } {
        crate::acp::event(s, Some(sid.to_owned()), kind, String::new());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_status_and_bounded_fragmented_sse() {
        assert_eq!(status(None).unwrap(), Status::Unknown);
        assert_eq!(
            status(Some(&serde_json::json!({"type":"future"}))).unwrap(),
            Status::Unknown
        );
        assert_eq!(
            status(Some(&serde_json::json!({"type":"retry"}))).unwrap(),
            Status::Working
        );
        assert!(status(Some(&Value::Null)).is_err());
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for b in b": comment\r\n\r\ndata: {\r\ndata: \"type\":\"server.connected\"}\r\n\r\n" {
            if let Some(v) = decoder.byte(*b).unwrap() {
                events.push(v);
            }
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "server.connected");
        for _ in 0..FRAME {
            decoder.byte(b'x').unwrap();
        }
        assert_eq!(decoder.byte(b'x').unwrap_err().0, "provider_limit");
        let mut bad = Decoder::default();
        assert!(b"data: nope\n\n".iter().any(|b| bad.byte(*b).is_err()));
    }
}
