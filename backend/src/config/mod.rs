use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::PathBuf};
use uuid::Uuid;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub root_path: PathBuf,
    pub git_common_dir: PathBuf,
    pub base_ref: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub projects: Vec<Project>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            projects: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != CONFIG_VERSION {
            return Err("Unsupported configuration version");
        }
        let mut ids = HashSet::new();
        let mut common_dirs = HashSet::new();
        for project in &self.projects {
            if project.name.trim().is_empty()
                || !project.root_path.is_absolute()
                || !project.git_common_dir.is_absolute()
                || project.id.is_nil()
            {
                return Err("Invalid project configuration");
            }
            if !ids.insert(project.id) || !common_dirs.insert(&project.git_common_dir) {
                return Err("Duplicate project configuration");
            }
        }
        Ok(())
    }
}
