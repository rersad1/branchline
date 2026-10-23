//! Fixed npm package entrypoints. Wrappers are located, never interpreted or executed.
use super::{executable, Error, NAMES};
use crate::environment::canonical_path;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Launch {
    pub node: PathBuf,
    pub package: PathBuf,
    pub entry: PathBuf,
    pub name: String,
}
#[derive(Deserialize)]
struct Manifest {
    candidates: Vec<Candidate>,
}
#[derive(Deserialize)]
pub struct Candidate {
    pub name: String,
    package: String,
    pub version: String,
    entry: String,
    pub node_major_min: u32,
    dependencies: BTreeMap<String, String>,
}
fn candidates() -> Vec<Candidate> {
    serde_json::from_str::<Manifest>(include_str!("../../../scripts/d7-candidates.json"))
        .expect("bundled candidate manifest")
        .candidates
}
fn json(path: &Path) -> Result<serde_json::Value, Error> {
    use std::io::Read;
    let metadata = std::fs::metadata(path).map_err(|_| Error("harness_package"))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(Error("harness_package"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| Error("harness_package"))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error("harness_package"))?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error("harness_package"));
    }
    serde_json::from_slice(&bytes).map_err(|_| Error("harness_package"))
}
pub fn validate(launch: &Launch) -> Result<Candidate, Error> {
    let c = candidates()
        .into_iter()
        .find(|c| c.name == launch.name)
        .ok_or(Error("harness_package"))?;
    let root = canonical_path(&launch.package).map_err(|_| Error("harness_package"))?;
    let p = json(&root.join("package.json"))?;
    if root != launch.package
        || p["name"] != c.package
        || p["version"] != c.version
        || p["bin"][&c.name] != c.entry
    {
        return Err(Error("harness_package"));
    }
    let entry = canonical_path(&root.join(&c.entry)).map_err(|_| Error("harness_package"))?;
    if entry != launch.entry
        || !entry.starts_with(&root)
        || !entry.is_file()
        || executable(&launch.node)? != launch.node
    {
        return Err(Error("harness_package"));
    }
    if launch.node.file_stem().and_then(|s| s.to_str()) != Some("node") {
        return Err(Error("harness_node"));
    }
    for (name, version) in &c.dependencies {
        let mut dir = launch.entry.parent().ok_or(Error("harness_package"))?;
        let dependency = loop {
            let path = dir.join("node_modules").join(name).join("package.json");
            if path.exists() {
                break json(&path)?;
            }
            dir = dir.parent().ok_or(Error("harness_package"))?;
        };
        if dependency["name"] != *name || dependency["version"] != *version {
            return Err(Error("harness_package"));
        }
    }
    if c.name == "codex-acp" && std::env::var_os("CODEX_PATH").is_some() {
        return Err(Error("harness_package"));
    }
    Ok(c)
}
pub fn node_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|p| p.is_absolute())
        .take(96)
        .find_map(|p| executable(&p.join(if cfg!(windows) { "node.exe" } else { "node" })).ok())
}
pub fn resolve(path: &Path, node: Option<&Path>) -> Result<Launch, Error> {
    if !path.is_absolute() || path.as_os_str().len() > 4096 {
        return Err(Error("harness_path"));
    }
    // A package directory, a published entrypoint, or a conventional npm alias.
    let canonical = canonical_path(path).map_err(|_| Error("harness_path"))?;
    let mut roots = Vec::new();
    if canonical.is_dir() {
        roots.push(canonical.clone());
    }
    if canonical.is_file() {
        if let Some(parent) = canonical.parent() {
            roots.push(parent.to_path_buf());
            if let Some(p) = parent.parent() {
                roots.push(p.to_path_buf());
            }
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if NAMES.contains(&stem) {
            if let Some(parent) = path.parent() {
                for c in candidates().into_iter().filter(|c| c.name == stem) {
                    // Local .bin, Windows global npm prefix, Unix global prefix/bin.
                    roots.push(parent.join("node_modules").join(&c.package));
                    if let Some(p) = parent.parent() {
                        if parent.file_name().is_some_and(|n| n == ".bin") {
                            roots.push(p.join(&c.package));
                        }
                        roots.push(p.join("lib/node_modules").join(&c.package));
                    }
                }
            }
        }
    }
    let node = match node {
        Some(p) => executable(p)?,
        None => node_on_path().ok_or(Error("harness_node"))?,
    };
    for root in roots {
        let Ok(root) = canonical_path(&root) else {
            continue;
        };
        let Ok(p) = json(&root.join("package.json")) else {
            continue;
        };
        if let Some(c) = candidates().into_iter().find(|c| p["name"] == c.package) {
            let entry =
                canonical_path(&root.join(&c.entry)).map_err(|_| Error("harness_package"))?;
            // Alias must resolve to the package entry on Unix; a Windows .cmd is
            // only a locator for the fixed manifest, never trusted as argv/code.
            if cfg!(unix) && canonical.is_file() && canonical != entry {
                return Err(Error("harness_package"));
            }
            let launch = Launch {
                node: node.clone(),
                package: root.clone(),
                entry,
                name: c.name,
            };
            validate(&launch)?;
            return Ok(launch);
        }
    }
    Err(Error("harness_package"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture(name: &str) -> (tempfile::TempDir, Launch) {
        let dir = tempfile::tempdir().unwrap();
        let c = candidates().into_iter().find(|c| c.name == name).unwrap();
        let root = dir.path().join("node_modules").join(&c.package);
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::create_dir_all(root.join("bundle")).unwrap();
        std::fs::write(
            root.join("package.json"),
            json!({"name":c.package,"version":c.version,"bin":{name:c.entry}}).to_string(),
        )
        .unwrap();
        std::fs::write(root.join(&c.entry), "// fixture, never executed").unwrap();
        for (dependency, version) in &c.dependencies {
            let d = dir.path().join("node_modules").join(dependency);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("package.json"),
                json!({"name":dependency,"version":version}).to_string(),
            )
            .unwrap();
        }
        let node = dir
            .path()
            .join(if cfg!(windows) { "node.exe" } else { "node" });
        std::fs::write(&node, "fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let launch = Launch {
            node: canonical_path(&node).unwrap(),
            package: canonical_path(&root).unwrap(),
            entry: canonical_path(&root.join(c.entry)).unwrap(),
            name: name.into(),
        };
        (dir, launch)
    }
    #[test]
    fn npm_package_alias_and_dependency_drift() {
        for name in ["gemini", "qwen", "codex-acp", "claude-agent-acp"] {
            let (dir, launch) = fixture(name);
            assert_eq!(
                resolve(&launch.package, Some(&launch.node)).unwrap(),
                launch
            );
            assert_eq!(resolve(&launch.entry, Some(&launch.node)).unwrap(), launch);
            let bin = dir.path().join("node_modules/.bin");
            std::fs::create_dir(&bin).unwrap();
            let alias = bin.join(if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.into()
            });
            #[cfg(windows)]
            std::fs::write(&alias, "@echo NEVER EXECUTE THIS WRAPPER").unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&launch.entry, &alias).unwrap();
            assert_eq!(resolve(&alias, Some(&launch.node)).unwrap(), launch);
            let c = validate(&launch).unwrap();
            if let Some((dep, _)) = c.dependencies.first_key_value() {
                let p = dir
                    .path()
                    .join("node_modules")
                    .join(dep)
                    .join("package.json");
                std::fs::write(p, json!({"name":dep,"version":"0.0.0"}).to_string()).unwrap();
                assert!(validate(&launch).is_err());
            }
            std::fs::write(launch.package.join("package.json"), "{}").unwrap();
            assert!(validate(&launch).is_err());
        }
    }
    #[test]
    fn npm_rejects_unknown_relative_and_changed_entry() {
        let (_dir, mut launch) = fixture("qwen");
        assert!(resolve(Path::new("relative.cmd"), Some(&launch.node)).is_err());
        launch.entry = launch.node.clone();
        assert!(validate(&launch).is_err());
        assert!(resolve(&launch.node, Some(&launch.node)).is_err());
    }
    #[test]
    fn npm_rejects_dependency_shadowing_next_to_entrypoint() {
        for name in ["codex-acp", "claude-agent-acp"] {
            let (_dir, launch) = fixture(name);
            let c = validate(&launch).unwrap();
            let (dep, _) = c.dependencies.first_key_value().unwrap();
            let shadow = launch
                .entry
                .parent()
                .unwrap()
                .join("node_modules")
                .join(dep);
            std::fs::create_dir_all(&shadow).unwrap();
            std::fs::write(
                shadow.join("package.json"),
                json!({"name":dep,"version":"0.0.0"}).to_string(),
            )
            .unwrap();
            assert!(validate(&launch).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn npm_rejects_entry_symlink_escape() {
        let (dir, launch) = fixture("qwen");
        let outside = dir.path().join("outside.js");
        std::fs::write(&outside, "fixture").unwrap();
        std::fs::remove_file(&launch.entry).unwrap();
        std::os::unix::fs::symlink(&outside, &launch.entry).unwrap();
        assert!(resolve(&launch.package, Some(&launch.node)).is_err());
        assert!(validate(&launch).is_err());
    }
}
