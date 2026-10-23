use agent_workspace_backend::{
    projects::{NewProject, Registry},
    provider::{Request, Status},
    service::WorkspaceService,
};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
#[derive(Clone)]
struct Mock {
    mode: Arc<AtomicUsize>,
    dir: String,
}
async fn health(State(m): State<Mock>) -> Response {
    match m.mode.load(Ordering::SeqCst) {
        1 => StatusCode::UNAUTHORIZED.into_response(),
        2 => Json(json!({"healthy":true,"version":"2.0.0"})).into_response(),
        3 => ([(header::CONTENT_TYPE, "application/json")], "not json").into_response(),
        8 => {
            tokio::time::sleep(Duration::from_secs(20)).await;
            StatusCode::OK.into_response()
        }
        9 => (
            [(header::CONTENT_TYPE, "application/json")],
            "x".repeat(2 * 1024 * 1024 + 1),
        )
            .into_response(),
        10 => (
            StatusCode::FOUND,
            [(header::LOCATION, "http://example.invalid/credentials")],
        )
            .into_response(),
        _ => Json(json!({"healthy":true,"version":"1.19.0"})).into_response(),
    }
}
async fn path(State(m): State<Mock>) -> Json<serde_json::Value> {
    Json(json!({"directory":if m.mode.load(Ordering::SeqCst)==4 { "relative" } else { &m.dir }}))
}
async fn sessions(State(m): State<Mock>) -> Json<serde_json::Value> {
    Json(if m.mode.load(Ordering::SeqCst) == 5 {
        json!({})
    } else {
        json!([{"id":"fixture", "directory":m.dir}])
    })
}
async fn events(State(m): State<Mock>) -> Response {
    let value = match m.mode.load(Ordering::SeqCst) {
        6 => "",
        7 => "data: nope\n\n",
        _ => "data: {\"type\":\"server.connected\",\"properties\":{}}\n\n",
    };
    ([(header::CONTENT_TYPE, "text/event-stream")], value).into_response()
}
#[tokio::test]
async fn provider_boundary_errors_cancel_limits_scope_cleanup() {
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().join("repo space");
    std::fs::create_dir(&repo).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .arg(&repo)
        .output()
        .unwrap()
        .status
        .success());
    let service =
        WorkspaceService::new(Registry::open(fixture.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: repo.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let snapshot = service.worktrees.refresh(project.project.id).await.unwrap();
    let mode = Arc::new(AtomicUsize::new(0));
    let state = Mock {
        mode: mode.clone(),
        dir: snapshot.worktrees[0].path.to_str().unwrap().into(),
    };
    let app=Router::new().route("/global/health",get(health)).route("/path",get(path)).route("/session",get(sessions)).route("/session/status",get(||async{Json(json!({}))})).route("/event",get(events)).route("/doc",get(||async{Json(json!({"paths":{"/global/health":{"get":{}},"/path":{"get":{}},"/session":{"get":{}},"/session/status":{"get":{}},"/event":{"get":{}}}}))})).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let request = Request {
        project_id: project.project.id,
        worktree_id: snapshot.worktrees[0].id,
        request_id: uuid::Uuid::new_v4(),
        port,
        cancel: false,
    };
    let observation = service
        .provider
        .execute(&service, request.clone())
        .await
        .unwrap()
        .observation
        .unwrap();
    assert_eq!(observation.sessions[0].status, Status::Unknown);
    let mut early = request.clone();
    early.request_id = uuid::Uuid::new_v4();
    early.cancel = true;
    service
        .provider
        .execute(&service, early.clone())
        .await
        .unwrap();
    early.cancel = false;
    assert_eq!(
        service
            .provider
            .execute(&service, early)
            .await
            .unwrap_err()
            .0,
        "provider_cancelled"
    );
    for (m, code) in [
        (1, "provider_auth"),
        (2, "provider_incompatible"),
        (3, "provider_malformed"),
        (4, "provider_scope"),
        (5, "provider_malformed"),
        (6, "provider_disconnect"),
        (7, "provider_malformed"),
        (9, "provider_limit"),
        (10, "provider_incompatible"),
    ] {
        mode.store(m, Ordering::SeqCst);
        assert_eq!(
            service
                .provider
                .execute(&service, request.clone())
                .await
                .unwrap_err()
                .0,
            code,
            "mode {m}"
        );
    }
    mode.store(8, Ordering::SeqCst);
    let mut pending = Vec::new();
    for _ in 0..4 {
        let s = service.clone();
        let mut r = request.clone();
        r.request_id = uuid::Uuid::new_v4();
        let cancel = r.clone();
        pending.push((
            cancel,
            tokio::spawn(async move { s.provider.execute(&s, r).await }),
        ));
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        service
            .provider
            .execute(&service, request.clone())
            .await
            .unwrap_err()
            .0,
        "provider_limit"
    );
    for (mut r, handle) in pending {
        r.cancel = true;
        service.provider.execute(&service, r).await.unwrap();
        assert_eq!(handle.await.unwrap().unwrap_err().0, "provider_cancelled");
    }
    mode.store(0, Ordering::SeqCst);
    service
        .provider
        .execute(&service, request.clone())
        .await
        .unwrap();
    let mut foreign = request.clone();
    foreign.worktree_id = uuid::Uuid::new_v4();
    assert_eq!(
        service
            .provider
            .execute(&service, foreign)
            .await
            .unwrap_err()
            .0,
        "provider_scope"
    );
    server.abort();
    let _ = server.await;
    assert_eq!(
        service
            .provider
            .execute(&service, request)
            .await
            .unwrap_err()
            .0,
        "provider_unavailable"
    );
}
