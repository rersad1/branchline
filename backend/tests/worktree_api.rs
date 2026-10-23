mod common;
use agent_workspace_backend::{
    app::{AppState, LocalAuth},
    projects::{NewProject, Registry},
    router,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use common::{git, Repositories};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn worktree_and_status_routes_require_auth_and_refresh_external_changes() {
    let repos = Repositories::new();
    assert!(repos.linked.join(".git").is_file());
    let registry = Registry::open(repos.directory.path().join("config.json")).unwrap();
    let project = registry
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let token = "a".repeat(32);
    let app = router(AppState::new(
        registry,
        LocalAuth::new(token.clone(), 43871).unwrap(),
    ));
    let path = format!("/api/v1/projects/{}/worktrees", project.id);
    let response = app
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let id = snapshot["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tree| tree["is_main"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let status_path = format!("/api/v1/worktrees/{id}/status");
    std::fs::write(repos.main.join("untracked"), "fixture").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&status_path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let status: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(status["untracked"], json!(1));
    let external = repos.directory.path().join("external");
    git(
        &repos.main,
        &[
            "worktree",
            "add",
            "-b",
            "external",
            external.to_str().unwrap(),
        ],
    );
    for origin in ["https://hostile.example", "http://localhost:43871"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{path}/refresh"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("origin", origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if origin.starts_with("https") {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            let snapshot: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(snapshot["worktrees"].as_array().unwrap().len(), 3);
        }
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/worktrees/00000000-0000-0000-0000-000000000001/status")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
