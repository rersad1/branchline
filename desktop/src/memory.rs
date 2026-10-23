//! Local opt-in recorder. One sample in RAM, two bounded JSONL files; export streams to disk.
use agent_workspace_backend::memory::Snapshot;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub const INTERVAL_SECONDS: u64 = 30;
const LOG_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentSnapshot {
    pub environment_id: Uuid,
    pub distro: Option<String>,
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Record {
    pub record_type: &'static str,
    pub schema_version: u8,
    pub captured_unix_ms: u64,
    pub collection_duration_ms: u64,
    pub host: Option<Snapshot>,
    pub environments: Vec<EnvironmentSnapshot>,
    pub errors: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub retained_bytes: u64,
    pub max_retained_bytes: u64,
    pub directory: Option<String>,
    pub export_path: Option<String>,
    pub error: Option<String>,
    pub last_sample: Option<Record>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Start,
    Stop,
    Export,
}
struct State {
    enabled: bool,
    error: Option<String>,
    last: Option<Record>,
    export_path: Option<String>,
}
pub struct Recorder {
    directory: Option<PathBuf>,
    state: Mutex<State>,
    pub wake: tokio::sync::Notify,
    limit: u64,
}
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Recorder {
    pub fn new() -> Self {
        Self::at(
            agent_workspace_backend::storage::default_config_path()
                .ok()
                .filter(|p| p.is_absolute())
                .map(|p| p.with_extension("memory-diagnostics")),
            LOG_LIMIT,
        )
    }
    fn at(directory: Option<PathBuf>, limit: u64) -> Self {
        let enabled = directory
            .as_ref()
            .is_some_and(|p| p.join("enabled").is_file());
        Self {
            directory,
            state: Mutex::new(State {
                enabled,
                error: None,
                last: None,
                export_path: None,
            }),
            wake: Default::default(),
            limit,
        }
    }
    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }
    pub fn action(&self, action: Action) -> Result<Status, &'static str> {
        let mut state = self.state.lock().unwrap();
        match action {
            Action::Status => {}
            Action::Start => {
                if state.enabled {
                    return Ok(self.status(&state));
                }
                let directory = self
                    .directory
                    .as_ref()
                    .ok_or("Каталог диагностики недоступен.")?;
                fs::create_dir_all(directory)
                    .map_err(|_| "Не удалось создать каталог диагностики.")?;
                File::create(directory.join("enabled"))
                    .map_err(|_| "Не удалось включить запись диагностики.")?;
                state.enabled = true;
                state.error = None;
                self.wake.notify_one();
            }
            Action::Stop => {
                if let Some(directory) = &self.directory {
                    match fs::remove_file(directory.join("enabled")) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(_) => return Err("Не удалось сохранить остановку диагностики."),
                    }
                }
                state.enabled = false;
            }
            Action::Export => {
                let directory = self
                    .directory
                    .as_ref()
                    .ok_or("Каталог диагностики недоступен.")?;
                fs::create_dir_all(directory).map_err(|_| "Каталог диагностики недоступен.")?;
                let path = directory.join(format!(
                    "memory-report-{}-{}.jsonl",
                    unix_ms(),
                    Uuid::new_v4()
                ));
                let result = (|| -> io::Result<()> {
                    let mut output = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&path)?;
                    serde_json::to_writer(
                        &mut output,
                        &serde_json::json!({
                            "record_type":"metadata", "schema_version":1, "application_version":env!("CARGO_PKG_VERSION"),
                            "exported_unix_ms":unix_ms(), "interval_seconds":INTERVAL_SECONDS,
                            "max_retained_bytes":self.limit*2,
                            "semantics": "All memory in bytes. Linux private_bytes=USS; Windows private_bytes=private commit, not resident RAM. RSS includes shared pages; never sum Windows Vmmem with Linux processes. PSS only covers readable listed processes. Linux meminfo is VM-wide and may cover other WSL2 distros. Other WSL distros are not started or enumerated. Names are basenames, categories are name heuristics; no argv, environment, paths, terminal text or prompts.",
                            "retention": "Two rotating logs; older samples may be absent. Manual exports remain until removed by the user."
                        }),
                    )?;
                    output.write_all(b"\n")?;
                    for name in ["memory-previous.jsonl", "memory-current.jsonl"] {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(directory.join(name))
                        {
                            Ok(mut input) => {
                                repair_tail(&mut input)?;
                                input.rewind()?;
                                io::copy(&mut input, &mut output)?;
                            }
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                            Err(e) => return Err(e),
                        }
                    }
                    output.sync_all()
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&path);
                    return Err("Не удалось экспортировать отчёт диагностики.");
                }
                state.export_path = Some(path.to_string_lossy().into_owned());
            }
        }
        Ok(self.status(&state))
    }
    fn status(&self, state: &State) -> Status {
        let retained_bytes = self.directory.as_ref().map_or(0, |p| {
            ["memory-current.jsonl", "memory-previous.jsonl"]
                .iter()
                .filter_map(|name| fs::metadata(p.join(name)).ok())
                .map(|m| m.len())
                .sum()
        });
        Status {
            enabled: state.enabled,
            interval_seconds: INTERVAL_SECONDS,
            retained_bytes,
            max_retained_bytes: self.limit * 2,
            directory: self
                .directory
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            export_path: state.export_path.clone(),
            error: state.error.clone(),
            last_sample: state.last.clone(),
        }
    }
    pub fn append(&self, record: Record) {
        let mut state = self.state.lock().unwrap();
        if !state.enabled {
            return;
        }
        let result = (|| -> io::Result<()> {
            let directory = self.directory.as_ref().ok_or(io::ErrorKind::NotFound)?;
            let bytes = serde_json::to_vec(&record)?;
            if bytes.len() as u64 + 1 > self.limit {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let current = directory.join("memory-current.jsonl");
            if fs::metadata(&current).map_or(0, |m| m.len()) + bytes.len() as u64 + 1 > self.limit {
                let previous = directory.join("memory-previous.jsonl");
                match fs::remove_file(&previous) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                fs::rename(&current, previous)?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(current)?;
            repair_tail(&mut file)?;
            file.seek(SeekFrom::End(0))?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.flush()
        })();
        state.error = result.err().map(|_| {
            "Ошибка записи журнала памяти; проверьте свободное место и права доступа.".into()
        });
        state.last = Some(record);
    }
}

// Recover complete records if a crash interrupted the last append.
fn repair_tail(file: &mut File) -> io::Result<()> {
    let mut end = file.metadata()?.len();
    if end == 0 {
        return Ok(());
    }
    let mut block = [0u8; 4096];
    while end > 0 {
        let start = end.saturating_sub(block.len() as u64);
        let count = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut block[..count])?;
        if let Some(index) = block[..count].iter().rposition(|b| *b == b'\n') {
            let complete = start + index as u64 + 1;
            if complete != file.metadata()?.len() {
                file.set_len(complete)?;
            }
            return Ok(());
        }
        end = start;
    }
    file.set_len(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovers_an_incomplete_record_after_a_crash() {
        let directory =
            std::env::temp_dir().join(format!("branchline-memory-tail-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("memory-current.jsonl"),
            b"{\"complete\":true}\n{\"partial\":",
        )
        .unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.join("memory-current.jsonl"))
            .unwrap();
        repair_tail(&mut file).unwrap();
        drop(file);
        assert_eq!(
            fs::read_to_string(directory.join("memory-current.jsonl")).unwrap(),
            "{\"complete\":true}\n"
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn recording_is_opt_in_resumes_rotates_exports_and_stops() {
        let directory = std::env::temp_dir().join(format!("branchline-memory-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), 1400);
        let record = || Record {
            record_type: "sample",
            schema_version: 1,
            captured_unix_ms: unix_ms(),
            collection_duration_ms: 1,
            host: None,
            environments: vec![],
            errors: vec!["x".repeat(400)],
        };
        recorder.append(record());
        assert!(!directory.exists());
        recorder.action(Action::Start).unwrap();
        assert!(Recorder::at(Some(directory.clone()), 1400).enabled());
        for _ in 0..9 {
            recorder.append(record());
        }
        let status = recorder.action(Action::Export).unwrap();
        assert!(status.retained_bytes <= 2800);
        assert!(directory.join("memory-previous.jsonl").exists());
        let export = fs::read_to_string(status.export_path.unwrap()).unwrap();
        assert!(export.lines().count() >= 3);
        for line in export.lines() {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }
        recorder.action(Action::Stop).unwrap();
        let before = recorder.action(Action::Status).unwrap().retained_bytes;
        recorder.append(record());
        assert_eq!(
            recorder.action(Action::Status).unwrap().retained_bytes,
            before
        );
        assert!(!Recorder::at(Some(directory.clone()), 1400).enabled());
        fs::remove_dir_all(directory).unwrap();
    }
}
