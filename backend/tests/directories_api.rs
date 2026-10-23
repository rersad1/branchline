use agent_workspace_backend::{
    app::{AppState, LocalAuth},
    projects::Registry,
    router,
};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

#[tokio::test]
async fn directory_endpoint_keeps_auth_scope_and_strict_schema() {
    let fixture = tempfile::tempdir().unwrap();
    let state = AppState::new(
        Registry::open(fixture.path().join("config.json")).unwrap(),
        LocalAuth::new("a".repeat(32), 43871).unwrap(),
    );
    let id = state.service.environment.id;
    let app = router(state);
    let path = fixture.path().to_str().unwrap();
    for (auth, query, body, expected) in [
        (
            false,
            String::new(),
            serde_json::json!({"path":path,"offset":0}),
            401,
        ),
        (
            true,
            format!("?environment_id={}", uuid::Uuid::new_v4()),
            serde_json::json!({"path":path,"offset":0}),
            400,
        ),
        (
            true,
            format!("?environment_id={id}"),
            serde_json::json!({"path":path,"offset":0,"file":"secrets"}),
            if cfg!(unix) { 422 } else { 400 },
        ),
        (
            true,
            format!("?environment_id={id}"),
            serde_json::json!({"path":path,"offset":0}),
            if cfg!(unix) { 200 } else { 400 },
        ),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/directories{query}"))
            .header("Content-Type", "application/json");
        if auth {
            request = request.header("Authorization", format!("Bearer {}", "a".repeat(32)));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        if expected == 200 {
            let body = to_bytes(response.into_body(), 65536).await.unwrap();
            let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(page["environment_id"], id.to_string());
            assert!(page["entries"].as_array().unwrap().len() <= 50);
        }
    }
}
