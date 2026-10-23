mod common;

use agent_workspace_backend::{
    app::{AppState, LocalAuth},
    config::Project,
    projects::{NewProject, Registry, RegistryError},
    router,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use common::{git, Repositories};
use serde_json::{json, Value};

const TOKEN: &str = "test-token-012345678901234567890123456789";

fn app(registry: Registry) -> Router {
    router(AppState::new(
        registry,
        LocalAuth::new(TOKEN.into(), 43871).unwrap(),
    ))
}

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    if status != StatusCode::NO_CONTENT {
        assert_eq!(response.headers()["content-type"], "application/json");
    }
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn add_list_get_restart_remove_preserves_git_files() {
    let repos = Repositories::new();
    let config_path = repos.directory.path().join("config.json");
    std::fs::write(repos.main.join("dirty.txt"), "must survive unregister").unwrap();
    let before_head = std::fs::read(repos.main.join(".git/HEAD")).unwrap();
    let before_index = std::fs::read(repos.main.join(".git/index")).ok();
    let application = app(Registry::open(config_path.clone()).unwrap());
    let (status, value) = call(
        &application,
        "POST",
        "/api/v1/projects",
        json!({"root_path":repos.main, "name":"Проект", "base_ref":"main"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut stored = value.clone();
    assert!(stored
        .as_object_mut()
        .unwrap()
        .remove("environment_id")
        .is_some());
    let project: Project = serde_json::from_value(stored).unwrap();
    let uri = format!("/api/v1/projects/{}", project.id);
    assert_eq!(
        call(&application, "GET", &uri, Value::Null).await,
        (StatusCode::OK, value.clone())
    );
    assert_eq!(
        call(&application, "GET", "/api/v1/projects", Value::Null).await,
        (StatusCode::OK, json!([value]))
    );
    assert_eq!(
        call(
            &application,
            "POST",
            "/api/v1/projects",
            json!({"root_path":repos.linked})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    drop(application);
    let application = app(Registry::open(config_path.clone()).unwrap());
    let (_, mut restored) = call(&application, "GET", &uri, Value::Null).await;
    restored.as_object_mut().unwrap().remove("environment_id");
    assert_eq!(
        serde_json::from_value::<Project>(restored).unwrap(),
        project
    );
    assert_eq!(
        call(&application, "DELETE", &uri, Value::Null).await,
        (StatusCode::NO_CONTENT, Value::Null)
    );
    assert_eq!(
        call(&application, "GET", &uri, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&application, "DELETE", &uri, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        std::fs::read_to_string(repos.main.join("dirty.txt")).unwrap(),
        "must survive unregister"
    );
    assert!(repos.linked.join(".git").exists());
    assert_eq!(
        std::fs::read(repos.main.join(".git/HEAD")).unwrap(),
        before_head
    );
    assert_eq!(
        std::fs::read(repos.main.join(".git/index")).ok(),
        before_index
    );
    git(&repos.main, &["rev-parse", "--git-common-dir"]);
    drop(application);
    let registry = Registry::open(config_path).unwrap();
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn authorization_origin_and_request_errors_are_json() {
    use tower::ServiceExt;
    let directory = tempfile::tempdir().unwrap();
    let application = app(Registry::open(directory.path().join("config.json")).unwrap());
    for method in ["GET", "POST", "DELETE"] {
        let path = if method == "DELETE" {
            "/api/v1/projects/00000000-0000-0000-0000-000000000001"
        } else {
            "/api/v1/projects"
        };
        for (token, origin, expected) in [
            (None, None, StatusCode::UNAUTHORIZED),
            (Some("wrong-token"), None, StatusCode::UNAUTHORIZED),
            (
                Some(TOKEN),
                Some("https://hostile.example"),
                StatusCode::FORBIDDEN,
            ),
            (Some(TOKEN), Some("null"), StatusCode::FORBIDDEN),
        ] {
            let mut request = Request::builder().method(method).uri(path);
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let response = application
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()["content-type"], "application/json");
        }
    }
    for origin in ["http://127.0.0.1:43871", "http://localhost:43871"] {
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/projects")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for (body, expected) in [
        ("{broken".to_owned(), StatusCode::BAD_REQUEST),
        (
            json!({"root_path":"relative"}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"root_path":"/missing", "secret":"redacted"}).to_string(),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"root_path":"/missing", "name":" "}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        ("x".repeat(17 * 1024), StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/projects")
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["content-type"], "application/json");
    }
    assert_eq!(
        call(
            &application,
            "GET",
            "/api/v1/projects/not-a-uuid",
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&application, "PUT", "/api/v1/projects", Value::Null)
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        call(&application, "GET", "/api/v1/projects", Value::Null)
            .await
            .1,
        json!([])
    );
}

#[tokio::test]
async fn simultaneous_adds_and_config_owner_are_serialized() {
    let repos = Repositories::new();
    let path = repos.directory.path().join("config.json");
    let registry = Registry::open(path.clone()).unwrap();
    assert!(matches!(
        Registry::open(path.clone()),
        Err(RegistryError::Busy)
    ));
    let (main, linked) = tokio::join!(
        registry.add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None
        }),
        registry.add(NewProject {
            root_path: repos.linked.to_str().unwrap().into(),
            name: None,
            base_ref: None
        }),
    );
    assert_eq!(usize::from(main.is_ok()) + usize::from(linked.is_ok()), 1);
    assert!(matches!(
        main.as_ref().err().or(linked.as_ref().err()),
        Some(RegistryError::Project(
            agent_workspace_backend::projects::ProjectError::Duplicate
        ))
    ));
    assert_eq!(registry.list().await.unwrap().len(), 1);
    drop(registry);
    let registry = Registry::open(path).unwrap();
    assert_eq!(registry.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn failed_persistence_does_not_change_in_memory_registry() {
    let repos = Repositories::new();
    let path = repos.directory.path().join("config.json");
    let registry = Registry::open(path.clone()).unwrap();
    let project = registry
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let backup = repos.directory.path().join("original.json");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    let application = app(registry.clone());
    let (status, error) = call(
        &application,
        "DELETE",
        &format!("/api/v1/projects/{}", project.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error["error"]["code"], "internal_error");
    assert_eq!(registry.list().await.unwrap(), vec![project.clone()]);
    assert_eq!(std::fs::read(&backup).unwrap(), before);
    let other = repos.directory.path().join("other");
    std::fs::create_dir(&other).unwrap();
    git(&other, &["init"]);
    assert!(registry
        .add(NewProject {
            root_path: other.to_str().unwrap().into(),
            name: None,
            base_ref: None
        })
        .await
        .is_err());
    assert_eq!(registry.list().await.unwrap(), vec![project]);
}
