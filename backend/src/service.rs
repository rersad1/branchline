//! Shared environment-local boundary for HTTP now and desktop IPC later.
use crate::{
    environment::{ExecutionEnvironment, ProjectDto},
    projects::{NewProject, Registry, RegistryError},
    worktrees::WorktreeService,
};
#[derive(Clone)]
pub struct WorkspaceService {
    pub harnesses: crate::harness::Registry,
    pub agents: crate::acp::Client,
    pub provider: crate::provider::ProviderService,
    pub terminals: crate::terminal::TerminalService,
    pub registry: Registry,
    pub worktrees: WorktreeService,
    pub environment: ExecutionEnvironment,
    pub(crate) create_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}
impl WorkspaceService {
    pub async fn diff(
        &self,
        request: crate::diff::DiffRequest,
    ) -> Result<crate::diff::DiffResponse, crate::diff::DiffError> {
        crate::diff::read(self, request).await
    }
    pub fn new(registry: Registry) -> Self {
        Self {
            harnesses: Default::default(),
            agents: Default::default(),
            provider: Default::default(),
            create_lock: Default::default(),
            terminals: Default::default(),
            environment: registry.environment().clone(),
            worktrees: WorktreeService::new(registry.clone()),
            registry,
        }
    }
    pub async fn remove(&self, id: uuid::Uuid) -> Result<(), RegistryError> {
        let _write = self.create_lock.lock().await;
        if self.terminals.active_project(id) || self.agents.active(id, None) {
            return Err(RegistryError::ActiveTerminal);
        }
        self.registry.remove(id).await
    }
    pub async fn projects(&self) -> Result<Vec<ProjectDto>, RegistryError> {
        Ok(self
            .registry
            .list()
            .await?
            .into_iter()
            .map(|project| self.project_dto(project))
            .collect())
    }
    pub fn project_dto(&self, project: crate::config::Project) -> ProjectDto {
        ProjectDto {
            environment_id: self.environment.id,
            project,
        }
    }
    pub async fn add(&self, request: NewProject) -> Result<ProjectDto, RegistryError> {
        Ok(self.project_dto(self.registry.add(request).await?))
    }
}
