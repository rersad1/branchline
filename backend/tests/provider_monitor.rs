//! Focused native adapter reconnect/resnapshot; real local sockets and isolated Git scope.
use agent_workspace_backend::{
    acp::{Action, Activity, Request},
    projects::{NewProject, Registry},
    service::WorkspaceService,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[tokio::test]
async fn native_monitor_reconnect_resnapshots_without_replay_or_ownership() {
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().join("repo");
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
    let p = service
        .add(NewProject {
            root_path: repo.to_str().unwrap().into(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let snapshot = service.worktrees.refresh(p.project.id).await.unwrap();
    let tree = &snapshot.worktrees[0];
    let phase = Arc::new(AtomicUsize::new(0));
    let mode = phase.clone();
    let dir = tree.path.to_str().unwrap().to_owned();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {r=listener.accept()=>{let (mut socket,_)=r.unwrap();let phase=mode.clone();let dir=dir.clone();tasks.spawn(async move{
             let mut bytes=[0;8192];let n=socket.read(&mut bytes).await.unwrap();let input=String::from_utf8_lossy(&bytes[..n]);let route=input.split_whitespace().nth(1).unwrap().split('?').next().unwrap();
             if route=="/event"{
              socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();let data="data: {\"type\":\"server.connected\"}\n\n";let chunk=format!("{:x}\r\n{data}\r\n",data.len());if socket.write_all(chunk.as_bytes()).await.is_err(){return;}
              while phase.load(Ordering::SeqCst)!=1{tokio::time::sleep(std::time::Duration::from_millis(30)).await;if socket.write_all(b"3\r\n:\n\n\r\n").await.is_err(){return;}}return;
             }
             let value=match route{
              "/global/health"=>json!({"healthy":phase.load(Ordering::SeqCst)!=1,"version":"1.19.0"}),
              "/path"=>json!({"directory":dir}),"/session"=>json!([{"id":"fixture","directory":dir,"parentID":"parent"}]),
              "/session/status"=>if phase.load(Ordering::SeqCst)==0{json!({"fixture":{"type":"busy"}})}else{json!({})},
              "/doc"=>json!({"paths":{"/global/health":{"get":{}},"/path":{"get":{}},"/session":{"get":{}},"/session/status":{"get":{}},"/event":{"get":{}}}}),_=>json!({})};
             let body=value.to_string();let out=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());let _=socket.write_all(out.as_bytes()).await;
            });},_=tasks.join_next(),if !tasks.is_empty()=>{}}
        }
    });
    let id = uuid::Uuid::new_v4();
    let request = |action| Request {
        project_id: p.project.id,
        worktree_id: tree.id,
        request_id: uuid::Uuid::new_v4(),
        action,
    };
    service
        .agents
        .execute(
            &service,
            request(Action::Attach {
                port,
                instance_id: id,
            }),
        )
        .await
        .unwrap();
    for stage in 0..3 {
        if stage == 1 {
            phase.store(1, Ordering::SeqCst);
        }
        if stage == 2 {
            phase.store(2, Ordering::SeqCst);
        }
        tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                let result = service
                    .agents
                    .execute(&service, request(Action::List))
                    .await
                    .unwrap();
                let i = &result.instances[0];
                assert!(!i.owned);
                let done = match stage {
                    0 => {
                        i.connection == "connected"
                            && i.sessions.first().is_some_and(|s| {
                                s.activity == Activity::Working
                                    && s.parent_id.as_deref() == Some("parent")
                            })
                    }
                    1 => {
                        i.stale
                            && i.connection == "disconnected"
                            && i.sessions.iter().all(|s| s.activity == Activity::Unknown)
                    }
                    _ => {
                        !i.stale
                            && i.connection == "connected"
                            && i.sessions.iter().all(|s| s.activity == Activity::Unknown)
                    }
                };
                if done {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
        })
        .await
        .unwrap();
    }
    service.agents.close_all().await;
    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/global/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["healthy"], true);
    server.abort();
    let _ = server.await;
}
