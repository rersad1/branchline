//! Windows settings hold references only; projects remain in their local registry.
use agent_workspace_backend::{
    environment::ExecutionEnvironment,
    storage::{atomic_write, default_config_path},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentReference {
    pub environment: ExecutionEnvironment,
    pub registration_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Option<Uuid>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalTab {
    pub id: Uuid,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TerminalLaunch>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalLaunch {
    Shell,
    Harness { installation_id: Uuid },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalLayout {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub tabs: Vec<TerminalTab>,
    pub active_id: Option<Uuid>,
    pub split_id: Option<Uuid>,
    pub vertical: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    #[serde(default)]
    pub preferences: Preferences,
    pub explorer_visible: bool,
    pub explorer_width: u16,
    pub collapsed: Vec<String>,
    #[serde(default)]
    pub terminals: Vec<TerminalLayout>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            preferences: Preferences::default(),
            explorer_visible: true,
            explorer_width: 280,
            collapsed: vec![],
            terminals: vec![],
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub light_theme: bool,
    pub notifications: bool,
    pub system_notifications: bool,
    pub close_without_confirmation: bool,
    pub ide: std::collections::BTreeMap<Uuid, String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub version: u32,
    pub environments: Vec<EnvironmentReference>,
    pub selection: Option<Selection>,
    pub layout: Layout,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: 1,
            environments: vec![],
            selection: None,
            layout: Layout::default(),
        }
    }
}
impl Workspace {
    fn validate(&self) -> Result<(), ()> {
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        if self.version != 1
            || self.environments.len() > 4
            || !(220..=360).contains(&self.layout.explorer_width)
            || self.layout.collapsed.len() > 1024
        {
            return Err(());
        }
        if self.layout.terminals.len() > 32 {
            return Err(());
        }
        if self.layout.preferences.ide.len() > 5
            || self.layout.preferences.ide.iter().any(|(id, path)| {
                id.is_nil() || path.len() > 4096 || path.chars().any(char::is_control)
            })
        {
            return Err(());
        }
        let mut contexts = std::collections::HashSet::new();
        for t in &self.layout.terminals {
            let mut tabs = std::collections::HashSet::new();
            if t.environment_id.is_nil()
                || t.project_id.is_nil()
                || t.worktree_id.is_nil()
                || !contexts.insert((t.environment_id, t.project_id, t.worktree_id))
                || t.tabs.len() > 8
                || t.tabs.iter().any(|tab| {
                    tab.id.is_nil()
                        || !tabs.insert(tab.id)
                        || tab.title.is_empty()
                        || tab.title.len() > 80
                        || tab.title.chars().any(char::is_control)
                        || matches!(&tab.launch, Some(TerminalLaunch::Harness { installation_id }) if installation_id.is_nil())
                })
                || t.active_id.is_some_and(|id| !tabs.contains(&id))
                || t.split_id
                    .is_some_and(|id| !tabs.contains(&id) || Some(id) == t.active_id)
            {
                return Err(());
            }
        }
        for reference in &self.environments {
            let e = &reference.environment;
            let Some(distro) = &e.distro else {
                return Err(());
            };
            if e.kind != agent_workspace_backend::environment::EnvironmentKind::Wsl
                || e.id.is_nil()
                || distro.is_empty()
                || distro.len() > 128
                || distro.starts_with('-')
                || distro.chars().any(char::is_control)
                || reference.registration_id.is_empty()
                || reference.registration_id.len() > 256
                || !ids.insert(e.id)
                || !names.insert(distro)
            {
                return Err(());
            }
        }
        if self.selection.as_ref().is_some_and(|s| {
            s.environment_id.is_nil()
                || s.project_id.is_nil()
                || s.worktree_id.is_some_and(|id| id.is_nil())
        }) || self
            .layout
            .collapsed
            .iter()
            .any(|id| id.len() > 160 || id.chars().any(char::is_control))
        {
            return Err(());
        }
        Ok(())
    }
}
pub struct WorkspaceStore {
    path: PathBuf,
    _lock: File,
    pub value: Workspace,
}
impl Drop for WorkspaceStore {
    fn drop(&mut self) {
        let _ = self._lock.unlock();
    }
}
impl WorkspaceStore {
    pub fn open() -> Result<Self, ()> {
        Self::open_at(
            default_config_path()
                .map_err(|_| ())?
                .with_extension("workspace.json"),
        )
    }
    fn open_at(path: PathBuf) -> Result<Self, ()> {
        let parent = path.parent().ok_or(())?;
        fs::create_dir_all(parent).map_err(|_| ())?;
        let path = parent
            .canonicalize()
            .map_err(|_| ())?
            .join(path.file_name().ok_or(())?);
        let lock_path = path.with_extension("json.lock");
        for p in [&path, &lock_path] {
            if fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(());
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|_| ())?;
        lock.try_lock().map_err(|_| ())?;
        let value: Workspace = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| ())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Workspace::default(),
            Err(_) => return Err(()),
        };
        value.validate()?;
        Ok(Self {
            path,
            _lock: lock,
            value,
        })
    }
    pub fn save(&mut self, next: Workspace) -> Result<(), ()> {
        next.validate()?;
        atomic_write(
            &self.path,
            &serde_json::to_vec_pretty(&next).map_err(|_| ())?,
        )
        .map_err(|_| ())?;
        self.value = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_upgrade_legacy_and_preserve_layout() {
        let mut json = serde_json::to_value(Workspace::default()).unwrap();
        json["layout"]
            .as_object_mut()
            .unwrap()
            .remove("preferences");
        let mut value: Workspace = serde_json::from_value(json).unwrap();
        assert!(!value.layout.preferences.light_theme);
        assert!(!value.layout.preferences.close_without_confirmation);
        value.layout.explorer_width = 312;
        value
            .layout
            .preferences
            .ide
            .insert(Uuid::new_v4(), "C:\\IDE path\\editor.exe".into());
        value.layout.preferences.notifications = true;
        let restored: Workspace =
            serde_json::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(restored.layout.explorer_width, 312);
        assert!(restored.layout.preferences.notifications);
        assert!(restored.validate().is_ok());
        value
            .layout
            .preferences
            .ide
            .insert(Uuid::nil(), "invalid".into());
        assert!(value.validate().is_err());
    }
    #[test]
    fn terminal_layout_upgrades_legacy_defaults_and_validates_scoped_tabs() {
        let mut legacy = serde_json::to_value(Workspace::default()).unwrap();
        legacy["layout"]
            .as_object_mut()
            .unwrap()
            .remove("terminals");
        let mut workspace: Workspace = serde_json::from_value(legacy).unwrap();
        assert!(workspace.layout.terminals.is_empty());
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        workspace.layout.terminals.push(TerminalLayout {
            environment_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            tabs: vec![
                TerminalTab {
                    id: first,
                    title: "Shell 1".into(),
                    launch: Some(TerminalLaunch::Shell),
                },
                TerminalTab {
                    id: second,
                    title: "Shell 2".into(),
                    launch: None,
                },
            ],
            active_id: Some(first),
            split_id: Some(second),
            vertical: true,
        });
        assert!(workspace.validate().is_ok());
        let json = serde_json::to_vec(&workspace).unwrap();
        let mut restored: Workspace = serde_json::from_slice(&json).unwrap();
        assert!(restored.layout.terminals[0].vertical);
        assert_eq!(restored.layout.terminals[0].active_id, Some(first));
        restored.layout.terminals[0].split_id = Some(first);
        assert!(restored.validate().is_err());
    }
    #[test]
    fn terminal_launch_metadata_is_optional_and_validated() {
        let id = Uuid::new_v4();
        let legacy = serde_json::json!({"id":id,"title":"Legacy shell"});
        let tab: TerminalTab = serde_json::from_value(legacy.clone()).unwrap();
        assert!(tab.launch.is_none());
        assert_eq!(serde_json::to_value(&tab).unwrap(), legacy);
        let shell: TerminalTab = serde_json::from_value(
            serde_json::json!({"id":id,"title":"Shell","launch":{"kind":"shell"}}),
        )
        .unwrap();
        assert!(matches!(shell.launch, Some(TerminalLaunch::Shell)));
        let install = Uuid::new_v4();
        let fixture = serde_json::json!({"id":id,"title":"Harness","launch":{"kind":"harness","installation_id":install}});
        let harness: TerminalTab = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(&harness).unwrap(), fixture);
        assert!(serde_json::from_value::<TerminalTab>(serde_json::json!({"id":id,"title":"Bad","launch":{"kind":"command","executable":"arbitrary"}})).is_err());
        let mut workspace = Workspace::default();
        workspace.layout.terminals.push(TerminalLayout {
            environment_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            tabs: vec![TerminalTab {
                id,
                title: "Nil harness".into(),
                launch: Some(TerminalLaunch::Harness {
                    installation_id: Uuid::nil(),
                }),
            }],
            active_id: Some(id),
            split_id: None,
            vertical: false,
        });
        assert!(workspace.validate().is_err());
    }
    #[test]
    fn references_are_locked_atomic_versioned_and_preserve_selection() {
        let dir = std::env::temp_dir().join(format!("awm-settings-{}", Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("workspace.json");
        let mut store = WorkspaceStore::open_at(path.clone()).unwrap();
        assert!(WorkspaceStore::open_at(path.clone()).is_err());
        let mut next = store.value.clone();
        next.selection = Some(Selection {
            environment_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            worktree_id: Some(Uuid::new_v4()),
        });
        store.save(next.clone()).unwrap();
        next.version = 99;
        assert!(store.save(next).is_err());
        drop(store);
        let store = WorkspaceStore::open_at(path.clone()).unwrap();
        assert!(store.value.selection.is_some());
        drop(store);
        fs::write(&path, br#"{"version":99}"#).unwrap();
        assert!(WorkspaceStore::open_at(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), br#"{"version":99}"#);
        fs::remove_dir_all(dir).unwrap();
    }
}
