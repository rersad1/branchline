mod memory;
mod notifications;
mod onboarding;
mod runtime;
mod workspace;

#[tauri::command]
async fn memory_diagnostics(
    recorder: tauri::State<'_, Arc<memory::Recorder>>,
    action: memory::Action,
) -> Result<memory::Status, Problem> {
    let recorder = recorder.inner().clone();
    tokio::task::spawn_blocking(move || recorder.action(action))
        .await
        .map_err(|_| Problem::new("operation_failed"))?
        .map_err(|message| Problem {
            code: "memory_diagnostics_failed".into(),
            message: message.into(),
        })
}

#[tauri::command]
async fn notify_attention(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, Arc<Runtime>>,
    target: notifications::Target,
) -> Result<Option<String>, Problem> {
    let preferences = runtime.workspace().await?.layout.preferences;
    if !preferences.notifications
        || !preferences.system_notifications
        || target.session_id.len() > 256
    {
        return Err(Problem::new("invalid_request"));
    }
    let response = runtime
        .agents(
            target.environment_id,
            agent_workspace_backend::acp::Request {
                project_id: target.project_id,
                worktree_id: target.worktree_id,
                request_id: uuid::Uuid::new_v4(),
                action: agent_workspace_backend::acp::Action::List,
            },
        )
        .await?;
    let instance = response
        .instances
        .iter()
        .find(|i| {
            i.id == target.instance_id
                && i.connection == "connected"
                && !i.stale
                && i.sessions
                    .iter()
                    .any(|s| s.id == target.session_id && s.assigned)
        })
        .ok_or_else(|| Problem::new("invalid_request"))?;
    let event = instance
        .events
        .iter()
        .find(|e| {
            e.sequence == target.sequence
                && e.session_id.as_deref() == Some(target.session_id.as_str())
        })
        .ok_or_else(|| Problem::new("invalid_request"))?;
    let label = match event.kind.as_str() {
        "attention_waiting" => "Агент ожидает ответа",
        "attention_error" => "Ошибка агента",
        "turn_end" if event.text == "end_turn" => "Ответ агента завершён",
        _ => return Err(Problem::new("invalid_request")),
    };
    app.state::<notifications::Notifications>()
        .show(app.clone(), target, label)
        .map_err(|reason| Problem {
            code: "notifications_unavailable".into(),
            message: format!(
                "Windows notifications недоступны ({reason}). Attention остаётся в приложении."
            ),
        })
}
#[tauri::command]
async fn harnesses(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::harness::Request,
) -> Result<agent_workspace_backend::harness::Response, Problem> {
    runtime.harnesses(scope.environment_id, request).await
}
#[tauri::command]
async fn agents(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::acp::Request,
) -> Result<agent_workspace_backend::acp::Response, Problem> {
    runtime.agents(scope.environment_id, request).await
}
#[tauri::command]
async fn provider(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::provider::Request,
) -> Result<agent_workspace_backend::provider::Response, Problem> {
    runtime.provider(scope.environment_id, request).await
}
#[tauri::command]
async fn terminal(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::terminal::Request,
) -> Result<agent_workspace_backend::terminal::Response, Problem> {
    runtime.terminal(scope.environment_id, request).await
}
#[tauri::command]
async fn remove_worktree(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::remove_worktree::Request,
) -> Result<agent_workspace_backend::remove_worktree::Response, Problem> {
    runtime.remove_worktree(scope.environment_id, request).await
}
#[tauri::command]
async fn create_worktree(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::create_worktree::CreateRequest,
) -> Result<agent_workspace_backend::create_worktree::CreateResponse, Problem> {
    runtime.create_worktree(scope.environment_id, request).await
}
#[tauri::command]
async fn edit_file(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::edit::Request,
) -> Result<agent_workspace_backend::edit::Response, Problem> {
    runtime.edit_file(scope.environment_id, request).await
}
#[tauri::command]
async fn read_diff(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::diff::DiffRequest,
) -> Result<agent_workspace_backend::diff::DiffResponse, Problem> {
    runtime.diff(scope.environment_id, request).await
}
use agent_workspace_backend::{
    environment::{ExecutionEnvironment, ProjectDto},
    worktrees::Snapshot,
};
use runtime::{Problem, ProjectScope, Runtime, RuntimeState, Scope};
use std::sync::Arc;
use tauri::Emitter;
use tauri::Manager;

#[tauri::command]
fn close_application(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn list_wsl_distros() -> Result<Vec<String>, Problem> {
    onboarding::distros().await
}
#[tauri::command]
async fn pick_windows_folder(window: tauri::WebviewWindow) -> Result<Option<String>, Problem> {
    #[cfg(windows)]
    {
        let owner = window.hwnd().map_err(|_| Problem::new("dialog_failed"))?.0 as isize;
        tokio::task::spawn_blocking(move || onboarding::folder(owner))
            .await
            .map_err(|_| Problem::new("dialog_failed"))?
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err(Problem::new("dialog_failed"))
    }
}
#[tauri::command]
async fn add_project(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    root_path: String,
) -> Result<ProjectDto, Problem> {
    runtime.add_project(scope.environment_id, root_path).await
}
#[tauri::command]
async fn browse_directories(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
    request: agent_workspace_backend::directories::DirectoryRequest,
) -> Result<agent_workspace_backend::directories::DirectoryPage, Problem> {
    runtime.directories(scope.environment_id, request).await
}

#[tauri::command]
async fn runtime_environment(
    runtime: tauri::State<'_, Arc<Runtime>>,
) -> Result<ExecutionEnvironment, Problem> {
    runtime.local_environment()
}
#[tauri::command]
async fn runtime_states(
    runtime: tauri::State<'_, Arc<Runtime>>,
) -> Result<Vec<RuntimeState>, Problem> {
    Ok(runtime.states().await)
}
#[tauri::command]
async fn load_workspace(
    runtime: tauri::State<'_, Arc<Runtime>>,
) -> Result<workspace::Workspace, Problem> {
    runtime.workspace().await
}
#[tauri::command]
async fn save_workspace_ui(
    runtime: tauri::State<'_, Arc<Runtime>>,
    selection: Option<workspace::Selection>,
    layout: workspace::Layout,
) -> Result<(), Problem> {
    runtime.save_ui(selection, layout).await
}
#[tauri::command]
async fn relink_wsl(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, Arc<Runtime>>,
    environment_id: uuid::Uuid,
) -> Result<ExecutionEnvironment, Problem> {
    runtime.relink(&app, environment_id).await
}
#[tauri::command]
async fn remove_project(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: ProjectScope,
) -> Result<(), Problem> {
    runtime
        .remove_project(scope.environment_id, scope.project_id)
        .await
}
#[tauri::command]
async fn connect_wsl(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, Arc<Runtime>>,
    distro: String,
    expected_id: Option<uuid::Uuid>,
) -> Result<ExecutionEnvironment, Problem> {
    runtime.connect(&app, distro, expected_id).await
}
#[tauri::command]
async fn list_projects(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: Scope,
) -> Result<Vec<ProjectDto>, Problem> {
    runtime.projects(scope.environment_id).await
}
#[tauri::command]
async fn list_worktrees(
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: ProjectScope,
    refresh: Option<bool>,
) -> Result<Snapshot, Problem> {
    runtime
        .worktrees(
            scope.environment_id,
            scope.project_id,
            refresh.unwrap_or(false),
        )
        .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            memory_diagnostics,
            notify_attention,
            close_application,
            provider,
            harnesses,
            agents,
            terminal,
            remove_worktree,
            create_worktree,
            read_diff,
            edit_file,
            runtime_environment,
            runtime_states,
            connect_wsl,
            list_projects,
            list_worktrees,
            list_wsl_distros,
            pick_windows_folder,
            add_project,
            browse_directories,
            load_workspace,
            save_workspace_ui,
            relink_wsl,
            remove_project
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit("app-close-requested", ());
            }
        })
        .setup(|app| {
            app.manage(notifications::Notifications::default());
            let runtime = Arc::new(tauri::async_runtime::block_on(Runtime::new()));
            app.manage(runtime.clone());
            let recorder = Arc::new(memory::Recorder::new());
            app.manage(recorder.clone());
            let memory_runtime = runtime.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if recorder.enabled() {
                        let record = memory_runtime.memory_sample().await;
                        let output = recorder.clone();
                        let _ = tokio::task::spawn_blocking(move || output.append(record)).await;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(memory::INTERVAL_SECONDS)) => {},
                        _ = recorder.wake.notified() => {},
                    }
                }
            });
            let restored = runtime.clone();
            let restore_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                restored.restore(&restore_handle).await;
            });
            let refresh = runtime.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    refresh.refresh_local().await;
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    runtime.tick(&handle).await;
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("desktop startup failed");
    let closing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if closed.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            if !closing.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let runtime = handle.state::<Arc<Runtime>>().inner().clone();
                let handle = handle.clone();
                let closed = closed.clone();
                tauri::async_runtime::spawn(async move {
                    handle.state::<notifications::Notifications>().clear();
                    runtime.close().await;
                    closed.store(true, std::sync::atomic::Ordering::SeqCst);
                    handle.exit(0);
                });
            }
        }
    });
}
