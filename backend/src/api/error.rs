use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Harness(#[from] crate::harness::Error),
    #[error(transparent)]
    Provider(#[from] crate::provider::Error),
    #[error("Resource not found")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("Project already registered")]
    Duplicate,
    #[error("Local authorization required")]
    Unauthorized,
    #[error("Origin is not allowed")]
    ForbiddenOrigin,
    #[error("Internal operation failed")]
    Internal,
    #[error("Invalid JSON request")]
    InvalidJson(StatusCode),
    #[error("Method not allowed")]
    MethodNotAllowed,
    #[error(transparent)]
    Project(#[from] crate::projects::ProjectError),
    #[error(transparent)]
    Directory(#[from] crate::directories::DirectoryError),
    #[error(transparent)]
    Diff(#[from] crate::diff::DiffError),
    #[error(transparent)]
    Edit(#[from] crate::edit::Error),
    #[error("{}", crate::create_worktree::message(.0.0))]
    Create(crate::create_worktree::CreateError),
    #[error("{}", crate::remove_worktree::message(.0.0))]
    Remove(crate::remove_worktree::RemoveError),
    #[error("{}", crate::terminal::message(.0.0))]
    Terminal(crate::terminal::TerminalError),
}

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetails,
}

#[derive(Serialize)]
struct ErrorDetails {
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Edit(e) => (
                if e.0 == "edit_conflict" {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::BAD_REQUEST
                },
                e.0,
            ),
            Self::Harness(e) => (StatusCode::BAD_REQUEST, e.0),
            Self::Provider(e) => (StatusCode::BAD_REQUEST, e.0),
            Self::Remove(e) => (StatusCode::BAD_REQUEST, e.0),
            Self::Terminal(e) => (StatusCode::BAD_REQUEST, e.0),
            Self::Create(e) => (StatusCode::BAD_REQUEST, e.0),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Validation(_) => (StatusCode::BAD_REQUEST, "validation_error"),
            Self::Duplicate => (StatusCode::CONFLICT, "duplicate_project"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::ForbiddenOrigin => (StatusCode::FORBIDDEN, "forbidden_origin"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            Self::InvalidJson(status) => (*status, "invalid_json"),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
            Self::Directory(e) => (
                StatusCode::BAD_REQUEST,
                match e {
                    crate::directories::DirectoryError::InvalidPath => "invalid_path",
                    crate::directories::DirectoryError::Unavailable => "directory_unavailable",
                    crate::directories::DirectoryError::Limit => "directory_limit",
                },
            ),
            Self::Diff(e) => (
                match e {
                    crate::diff::DiffError::FileNotFound => StatusCode::NOT_FOUND,
                    crate::diff::DiffError::Timeout => StatusCode::GATEWAY_TIMEOUT,
                    crate::diff::DiffError::Git => StatusCode::SERVICE_UNAVAILABLE,
                    crate::diff::DiffError::Changed => StatusCode::CONFLICT,
                    _ => StatusCode::BAD_REQUEST,
                },
                e.code(),
            ),
            Self::Project(error) => {
                use crate::projects::ProjectError;
                match error {
                    ProjectError::Duplicate => (StatusCode::CONFLICT, "duplicate_project"),
                    ProjectError::GitUnavailable => {
                        (StatusCode::SERVICE_UNAVAILABLE, "git_unavailable")
                    }
                    ProjectError::GitTimeout => (StatusCode::GATEWAY_TIMEOUT, "git_timeout"),
                    ProjectError::Unavailable => (StatusCode::BAD_REQUEST, "project_unavailable"),
                    ProjectError::NotRepository => (StatusCode::BAD_REQUEST, "not_repository"),
                    ProjectError::InvalidPath | ProjectError::UnsupportedPath => {
                        (StatusCode::BAD_REQUEST, "invalid_path")
                    }
                }
            }
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorDetails {
                    code,
                    message: self.to_string(),
                },
            }),
        )
            .into_response()
    }
}

impl From<crate::projects::RegistryError> for ApiError {
    fn from(error: crate::projects::RegistryError) -> Self {
        use crate::projects::RegistryError;
        match error {
            RegistryError::ActiveTerminal => {
                Self::Terminal(crate::terminal::TerminalError("terminal_active"))
            }
            RegistryError::Project(error) => Self::Project(error),
            RegistryError::NotFound => Self::NotFound,
            RegistryError::InvalidInput(message) => Self::Validation(message.into()),
            other => {
                tracing::error!(error = %other, "registry operation failed");
                Self::Internal
            }
        }
    }
}
