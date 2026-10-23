pub mod api;
pub mod app;
pub mod config;
pub mod create_worktree;
pub mod git;
pub mod memory;
pub mod projects;
pub mod remove_worktree;
pub mod storage;
pub mod terminal;
pub mod terminal_shell;
pub mod worktrees;

pub fn router(state: app::AppState) -> axum::Router {
    api::router(state)
}

pub mod acp;
pub mod diff;
pub mod directories;
pub mod edit;
pub mod environment;
pub mod harness;
pub mod provider;
pub mod service;
pub mod worker;
