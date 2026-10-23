use std::{net::Ipv4Addr, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("--terminal-shell" | "--harness-stdio")
    ) {
        return ExitCode::from(agent_workspace_backend::terminal_shell::run() as u8);
    }
    if std::env::args().nth(1).as_deref() == Some("--desktop-worker") {
        return match agent_workspace_backend::worker::run().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "backend startup failed");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    use agent_workspace_backend::{
        app::{AppState, LocalAuth},
        projects::Registry,
        storage::default_config_path,
    };
    let port = std::env::var("AWM_PORT")
        .unwrap_or_else(|_| "43871".into())
        .parse::<u16>()?;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let auth = LocalAuth::new(
        std::env::var("AWM_TOKEN").map_err(|_| "AWM_TOKEN is required")?,
        listener.local_addr()?.port(),
    )?;
    let path = default_config_path()?;
    let registry = tokio::task::spawn_blocking(move || Registry::open(path)).await??;
    let state = AppState::new(registry, auth);
    let worktrees = state.worktrees.clone();
    let refresher = tokio::spawn(async move {
        worktrees
            .run_refresh_loop(std::time::Duration::from_secs(5))
            .await;
    });
    tracing::info!(address = %listener.local_addr()?, "backend listening");
    let result = axum::serve(listener, agent_workspace_backend::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    refresher.abort();
    let _ = refresher.await;
    result?;
    Ok(())
}
