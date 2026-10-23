use crate::{api::error::ApiError, projects::Registry};
use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};

#[derive(Clone)]
pub struct AppState {
    pub registry: Registry,
    pub service: crate::service::WorkspaceService,
    pub auth: LocalAuth,
    pub worktrees: crate::worktrees::WorktreeService,
}

impl AppState {
    pub fn new(registry: Registry, auth: LocalAuth) -> Self {
        let service = crate::service::WorkspaceService::new(registry.clone());
        let worktrees = service.worktrees.clone();
        Self {
            registry,
            service,
            auth,
            worktrees,
        }
    }
}

#[derive(Clone)]
pub struct LocalAuth {
    token: String,
    origins: [String; 2],
}

impl LocalAuth {
    pub fn new(token: String, port: u16) -> Result<Self, &'static str> {
        if !(32..=256).contains(&token.len())
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        {
            return Err("AWM_TOKEN must contain 32–256 ASCII letters, digits, '-' or '_'");
        }
        Ok(Self {
            token,
            origins: [
                format!("http://127.0.0.1:{port}"),
                format!("http://localhost:{port}"),
            ],
        })
    }
}

pub async fn authorize(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let headers = request.headers();
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1
        || !token_matches(authorization.as_bytes(), state.auth.token.as_bytes())
    {
        return Err(ApiError::Unauthorized);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        if headers.get_all(header::ORIGIN).iter().count() != 1
            || !state
                .auth
                .origins
                .iter()
                .any(|allowed| origin.as_bytes() == allowed.as_bytes())
        {
            return Err(ApiError::ForbiddenOrigin);
        }
    }
    #[derive(serde::Deserialize)]
    struct Scope {
        environment_id: Option<uuid::Uuid>,
    }
    let scope = axum::extract::Query::<Scope>::try_from_uri(request.uri())
        .map_err(|_| ApiError::Validation("Invalid environment identity".into()))?;
    if scope
        .environment_id
        .is_some_and(|id| id != state.service.environment.id)
    {
        return Err(ApiError::Validation(
            "Environment does not match this service".into(),
        ));
    }
    Ok(next.run(request).await)
}

fn token_matches(provided: &[u8], expected: &[u8]) -> bool {
    if provided.len() != expected.len() {
        return false;
    }
    provided
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}
