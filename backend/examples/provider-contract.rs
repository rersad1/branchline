//! Fixture-only contract driver. Caller owns isolated repo/provider; no provider control API.
use agent_workspace_backend::{
    projects::{NewProject, Registry},
    provider::Request,
    service::WorkspaceService,
};
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    let temp = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(Registry::open(temp.path().join("config.json")).unwrap());
    let project = service
        .add(NewProject {
            root_path: args[2].clone(),
            name: None,
            base_ref: None,
        })
        .await
        .unwrap();
    let snapshot = service.worktrees.refresh(project.project.id).await.unwrap();
    let path = agent_workspace_backend::environment::canonical_path(std::path::Path::new(&args[2]))
        .unwrap();
    let tree = snapshot.worktrees.iter().find(|t| t.path == path).unwrap();
    let request = Request {
        project_id: project.project.id,
        worktree_id: tree.id,
        request_id: uuid::Uuid::new_v4(),
        port: args[1].parse().unwrap(),
        cancel: false,
    };
    match service.provider.execute(&service, request).await {
        Ok(value) => println!("{}", serde_json::to_string(&value).unwrap()),
        Err(e) => {
            println!("{}", serde_json::json!({"error":e.0}));
            std::process::exit(1);
        }
    }
}
