pub mod error;

use crate::{
    app::{authorize, AppState},
    environment::ProjectDto,
    projects::NewProject,
};
use axum::{
    extract::{
        rejection::{JsonRejection, PathRejection},
        DefaultBodyLimit, Path, State,
    },
    http::StatusCode,
    middleware,
    routing::get,
    Json, Router,
};
use error::ApiError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize)]
struct Health {
    healthy: bool,
    version: &'static str,
}

pub fn router(state: AppState) -> Router {
    let projects = Router::new()
        .route("/api/v1/harnesses", axum::routing::post(harnesses))
        .route("/api/v1/agents", axum::routing::post(agents))
        .route("/api/v1/provider", axum::routing::post(provider))
        .route("/api/v1/environment", get(environment))
        .route(
            "/api/v1/worktrees/remove",
            axum::routing::post(remove_worktree),
        )
        .route("/api/v1/terminals", axum::routing::post(terminal))
        .route(
            "/api/v1/worktrees/create",
            axum::routing::post(create_worktree),
        )
        .route("/api/v1/diff", axum::routing::post(read_diff))
        .route(
            "/api/v1/directories",
            axum::routing::post(browse_directories),
        )
        .route("/api/v1/projects", get(list_projects).post(add_project))
        .route(
            "/api/v1/projects/{project_id}",
            get(get_project).delete(remove_project),
        )
        .route(
            "/api/v1/projects/{project_id}/worktrees",
            get(list_worktrees),
        )
        .route(
            "/api/v1/projects/{project_id}/worktrees/refresh",
            axum::routing::post(refresh_worktrees),
        )
        .route("/api/v1/worktrees/{worktree_id}", get(get_worktree))
        .route(
            "/api/v1/worktrees/{worktree_id}/status",
            get(get_git_status),
        )
        .route(
            "/api/v1/edit",
            axum::routing::post(edit_file).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .route_layer(middleware::from_fn_with_state(state.clone(), authorize));
    Router::new()
        .merge(projects)
        .route("/api/v1/health", get(health))
        .fallback(|| async { error::ApiError::NotFound })
        .method_not_allowed_fallback(|| async { error::ApiError::MethodNotAllowed })
        .with_state(state)
}

async fn provider(
    State(state): State<AppState>,
    request: Result<Json<crate::provider::Request>, JsonRejection>,
) -> Result<Json<crate::provider::Response>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(
        state
            .service
            .provider
            .execute(&state.service, request)
            .await?,
    ))
}

async fn harnesses(
    State(state): State<AppState>,
    request: Result<Json<crate::harness::Request>, JsonRejection>,
) -> Result<Json<crate::harness::Response>, ApiError> {
    let Json(r) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(
        state.service.harnesses.execute(&state.service, r).await?,
    ))
}
async fn agents(
    State(state): State<AppState>,
    request: Result<Json<crate::acp::Request>, JsonRejection>,
) -> Result<Json<crate::acp::Response>, ApiError> {
    let Json(r) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(state.service.agents.execute(&state.service, r).await?))
}

async fn remove_worktree(
    State(state): State<AppState>,
    request: Result<Json<crate::remove_worktree::Request>, JsonRejection>,
) -> Result<Json<crate::remove_worktree::Response>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    crate::remove_worktree::remove(&state.service, request)
        .await
        .map(Json)
        .map_err(ApiError::Remove)
}
async fn terminal(
    State(state): State<AppState>,
    request: Result<Json<crate::terminal::Request>, JsonRejection>,
) -> Result<Json<crate::terminal::Response>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    crate::terminal::execute(&state.service, request)
        .await
        .map(Json)
        .map_err(ApiError::Terminal)
}
async fn create_worktree(
    State(state): State<AppState>,
    request: Result<Json<crate::create_worktree::CreateRequest>, JsonRejection>,
) -> Result<Json<crate::create_worktree::CreateResponse>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    crate::create_worktree::create(&state.service, request)
        .await
        .map(Json)
        .map_err(ApiError::Create)
}

async fn edit_file(
    State(state): State<AppState>,
    request: Result<Json<crate::edit::Request>, JsonRejection>,
) -> Result<Json<crate::edit::Response>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(crate::edit::execute(&state.service, request).await?))
}
async fn read_diff(
    State(state): State<AppState>,
    request: Result<Json<crate::diff::DiffRequest>, JsonRejection>,
) -> Result<Json<crate::diff::DiffResponse>, ApiError> {
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(state.service.diff(request).await?))
}

async fn browse_directories(
    State(state): State<AppState>,
    request: Result<Json<crate::directories::DirectoryRequest>, JsonRejection>,
) -> Result<Json<crate::directories::DirectoryPage>, ApiError> {
    if state.service.environment.kind != crate::environment::EnvironmentKind::Wsl {
        return Err(ApiError::Validation(
            "Directory browsing requires WSL".into(),
        ));
    }
    let Json(request) = request.map_err(|e| ApiError::InvalidJson(e.status()))?;
    Ok(Json(
        crate::directories::browse(state.service.environment.id, request)
            .await
            .map_err(ApiError::Directory)?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddProject {
    root_path: String,
    name: Option<String>,
    base_ref: Option<String>,
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Vec<ProjectDto>>, ApiError> {
    Ok(Json(state.service.projects().await?))
}

async fn add_project(
    State(state): State<AppState>,
    request: Result<Json<AddProject>, JsonRejection>,
) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
    let Json(request) = request.map_err(|error| ApiError::InvalidJson(error.status()))?;
    let project = state
        .service
        .add(NewProject {
            root_path: request.root_path,
            name: request.name,
            base_ref: request.base_ref,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(project)))
}

fn project_id(path: Result<Path<Uuid>, PathRejection>) -> Result<Uuid, ApiError> {
    path.map(|Path(id)| id)
        .map_err(|_| ApiError::Validation("Invalid project ID".into()))
}

async fn get_project(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ProjectDto>, ApiError> {
    Ok(Json(
        state
            .service
            .project_dto(state.registry.get(project_id(path)?).await?),
    ))
}

async fn remove_project(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    state.service.remove(project_id(path)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn health() -> Json<Health> {
    Json(Health {
        healthy: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn list_worktrees(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<crate::worktrees::Snapshot>, ApiError> {
    Ok(Json(state.worktrees.list(project_id(path)?).await?))
}

async fn refresh_worktrees(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<crate::worktrees::Snapshot>, ApiError> {
    Ok(Json(state.worktrees.refresh(project_id(path)?).await?))
}

async fn get_worktree(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<crate::worktrees::GitWorktree>, ApiError> {
    Ok(Json(state.worktrees.get(project_id(path)?).await?))
}

async fn get_git_status(
    State(state): State<AppState>,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<StatusDto>, ApiError> {
    let tree = state.worktrees.get(project_id(path)?).await?;
    Ok(Json(StatusDto {
        environment_id: tree.environment_id,
        worktree_id: tree.id,
        status: tree.git_status,
    }))
}

#[derive(Serialize)]
struct StatusDto {
    environment_id: Uuid,
    worktree_id: Uuid,
    #[serde(flatten)]
    status: crate::git::status::GitStatus,
}

async fn environment(
    State(state): State<AppState>,
) -> Json<crate::environment::ExecutionEnvironment> {
    Json(state.service.environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_and_missing_route_have_json_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::projects::Registry::open(directory.path().join("config.json")).unwrap(),
            crate::app::LocalAuth::new("a".repeat(32), 43871).unwrap(),
        );
        for (path, status, field, value) in [
            (
                "/api/v1/health",
                StatusCode::OK,
                "healthy",
                serde_json::json!(true),
            ),
            (
                "/missing",
                StatusCode::NOT_FOUND,
                "error",
                serde_json::json!({"code":"not_found", "message":"Resource not found"}),
            ),
        ] {
            let response = router(state.clone())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["content-type"], "application/json");
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json[field], value);
        }
    }
}
