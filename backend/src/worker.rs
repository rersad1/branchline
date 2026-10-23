//! Private desktop bootstrap. No secret in argv, environment, readiness or errors.
use crate::environment::ExecutionEnvironment;
#[cfg(unix)]
use crate::{
    app::{AppState, LocalAuth},
    projects::Registry,
    storage::default_config_path,
};
#[cfg(unix)]
use axum::{extract::State, middleware, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
#[cfg(unix)]
use tokio::io::AsyncWriteExt;

pub const PROTOCOL: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Startup {
    pub token: String,
    pub nonce: String,
    pub protocol: u32,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ready {
    pub port: u16,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Handshake {
    pub protocol: u32,
    pub version: String,
    pub nonce: String,
    pub environment: ExecutionEnvironment,
}

/// Read exactly one bounded line, without buffering bytes belonging to the lease pipe.
pub async fn read_line<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let byte = reader.read_u8().await?;
        if byte == b'\n' {
            return Ok(bytes);
        }
        if bytes.len() == limit {
            return Err(std::io::Error::other("Startup frame too large"));
        }
        bytes.push(byte);
    }
}

#[cfg(unix)]
struct LeasePipe(tokio::io::unix::AsyncFd<std::fs::File>);
#[cfg(unix)]
impl LeasePipe {
    fn stdin() -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;
        // SAFETY: duplicate stdin is uniquely owned; nonblocking pipe never uses Tokio's
        // uncancellable blocking stdin thread, which would hang lease expiry shutdown.
        let file = unsafe {
            let fd = libc::dup(0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let file = std::fs::File::from_raw_fd(fd);
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            file
        };
        Ok(Self(tokio::io::unix::AsyncFd::new(file)?))
    }
    async fn byte(&self) -> std::io::Result<u8> {
        loop {
            let mut ready = self.0.readable().await?;
            match ready.try_io(|fd| {
                let mut byte = [0];
                if std::io::Read::read(&mut fd.get_ref(), &mut byte)? == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                Ok(byte[0])
            }) {
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }
    async fn frame(&self) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        loop {
            let byte = self.byte().await?;
            if byte == b'\n' {
                return Ok(bytes);
            }
            if bytes.len() == 1024 {
                return Err(std::io::Error::other("Startup frame too large"));
            }
            bytes.push(byte);
        }
    }
}
#[cfg(windows)]
pub async fn run() -> Result<(), &'static str> {
    Err("Desktop workers require Linux")
}
#[cfg(unix)]
pub async fn run() -> Result<(), &'static str> {
    let stdin = LeasePipe::stdin().map_err(|_| "Startup unavailable")?;
    let frame = tokio::time::timeout(Duration::from_secs(10), stdin.frame())
        .await
        .map_err(|_| "Startup timeout")?
        .map_err(|_| "Startup unavailable")?;
    let startup: Startup = serde_json::from_slice(&frame).map_err(|_| "Invalid startup")?;
    if startup.protocol != PROTOCOL
        || startup.nonce.len() != 64
        || !startup.nonce.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Incompatible startup");
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| "Loopback unavailable")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Loopback unavailable")?
        .port();
    let auth = LocalAuth::new(startup.token, port).map_err(|_| "Invalid authorization")?;
    let path = default_config_path().map_err(|_| "Registry unavailable")?;
    let registry = tokio::task::spawn_blocking(move || Registry::open(path))
        .await
        .map_err(|_| "Registry unavailable")?
        .map_err(|_| "Registry unavailable")?;
    let state = AppState::new(registry, auth);
    let handshake = Handshake {
        protocol: PROTOCOL,
        version: VERSION.into(),
        nonce: startup.nonce,
        environment: state.service.environment.clone(),
    };
    let lease = Arc::new(Mutex::new(Instant::now()));
    let lease_request = lease.clone();
    let private = Router::new()
        .route(
            "/api/v1/runtime/memory",
            get(|State(state): State<AppState>| async move {
                tokio::task::spawn_blocking(move || crate::memory::sample(&state.service.terminals))
                    .await
                    .map(Json)
                    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
            }),
        )
        .route(
            "/api/v1/runtime/handshake",
            get(move |State(_): State<AppState>| {
                let handshake = handshake.clone();
                *lease_request.lock().expect("lease") = Instant::now();
                async move { Json(handshake) }
            }),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::app::authorize,
        ))
        .with_state(state.clone());
    let worktrees = state.worktrees.clone();
    let refresher =
        tokio::spawn(async move { worktrees.run_refresh_loop(Duration::from_secs(5)).await });
    let ready = serde_json::to_vec(&Ready { port }).map_err(|_| "Ready unavailable")?;
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(&ready)
        .await
        .map_err(|_| "Ready unavailable")?;
    stdout
        .write_all(b"\n")
        .await
        .map_err(|_| "Ready unavailable")?;
    stdout.flush().await.map_err(|_| "Ready unavailable")?;
    // Pipe EOF closes normally; lease bounds orphan lifetime if an intermediary keeps it open.
    let terminals = state.service.terminals.clone();
    let agents = state.service.agents.clone();
    let server = axum::serve(listener, crate::router(state).merge(private));
    tokio::select! {
        _ = server => {},
        _ = stdin.byte() => {},
        _ = async { loop { tokio::time::sleep(Duration::from_secs(1)).await; if lease.lock().expect("lease").elapsed() > Duration::from_secs(10) { break; } } } => {},
    }
    agents.close_all().await;
    tokio::task::spawn_blocking(move || terminals.close_all())
        .await
        .map_err(|_| "Terminal cleanup failed")?;
    refresher.abort();
    let _ = refresher.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn startup_frame_is_bounded_and_does_not_consume_lease() {
        let mut input = &b"abc\nx"[..];
        assert_eq!(read_line(&mut input, 3).await.unwrap(), b"abc");
        assert_eq!(input.read_u8().await.unwrap(), b'x');
        assert!(read_line(&mut &b"abcd\n"[..], 3).await.is_err());
        assert!(read_line(&mut &b"abc"[..], 3).await.is_err());
    }
}
