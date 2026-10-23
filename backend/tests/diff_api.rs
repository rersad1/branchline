mod common;
use agent_workspace_backend::{
    app::{AppState, LocalAuth},
    diff::{DiffRequest, Mode},
    projects::{NewProject, Registry},
    router,
    service::WorkspaceService,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;
#[tokio::test]
async fn diff_route_requires_auth_scope_strict_schema_and_safe_ids() {
    let repos = common::Repositories::new();
    assert!(repos.linked.exists());
    std::fs::write(repos.main.join("new"), "content").unwrap();
    let service =
        WorkspaceService::new(Registry::open(repos.directory.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repos.main.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let tree = service
        .worktrees
        .refresh(project.project.id)
        .await
        .unwrap()
        .worktrees
        .into_iter()
        .find(|t| t.is_main)
        .unwrap();
    let token = "test-token-012345678901234567890123456789";
    let app = router(AppState::new(
        service.registry.clone(),
        LocalAuth::new(token.into(), 43871).unwrap(),
    ));
    let request = DiffRequest {
        project_id: project.project.id,
        worktree_id: tree.id,
        mode: Mode::Unstaged,
        base_ref: None,
        file_id: None,
        context_lines: 3,
    };
    let body = serde_json::to_value(request).unwrap();
    for (auth, scope, origin, extra, expected) in [
        (
            false,
            service.environment.id,
            None,
            false,
            StatusCode::UNAUTHORIZED,
        ),
        (
            true,
            uuid::Uuid::new_v4(),
            None,
            false,
            StatusCode::BAD_REQUEST,
        ),
        (
            true,
            service.environment.id,
            Some("null"),
            false,
            StatusCode::FORBIDDEN,
        ),
        (
            true,
            service.environment.id,
            None,
            true,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (true, service.environment.id, None, false, StatusCode::OK),
    ] {
        let mut value = body.clone();
        if extra {
            value["path"] = serde_json::json!("../../secret");
        }
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/diff?environment_id={scope}"))
            .header("content-type", "application/json");
        if auth {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(value.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), expected);
        let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(token));
        if expected == StatusCode::OK {
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(v["environment_id"], service.environment.id.to_string());
            assert_eq!(v["files"][0]["path"], "new");
            assert!(v["selected_file"].is_null());
        }
    }
}
