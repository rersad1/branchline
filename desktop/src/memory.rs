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
mod summary;
mod tracking;
pub use tracking::{BudgetSetting, BudgetStatus, Event};

pub const INTERVAL_SECONDS: u64 = 30;
const LOG_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentSnapshot {
    pub environment_id: Uuid,
    pub distro: Option<String>,
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Record {
    pub record_type: &'static str,
    pub schema_version: u8,
    pub captured_unix_ms: u64,
    pub collection_duration_ms: u64,
    pub host: Option<Snapshot>,
    pub environments: Vec<EnvironmentSnapshot>,
    pub errors: Vec<String>,
    pub host_environment_id: Uuid,
    pub session_id: Uuid,
    pub snapshot_bytes: usize,
    pub events: Vec<Event>,
    pub omitted_events: usize,
    pub budget_settings: Vec<BudgetSetting>,
    pub budgets: Vec<BudgetStatus>,
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
    pub budget_settings: Vec<BudgetSetting>,
    pub budgets: Vec<BudgetStatus>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Start,
    Stop,
    Export,
    ExportSummary,
    SetBudget,
}
struct State {
    enabled: bool,
    error: Option<String>,
    last: Option<Record>,
    export_path: Option<String>,
    budget_settings: Vec<BudgetSetting>,
    tracker: tracking::Tracker,
    interrupted: bool,
}
pub struct Recorder {
    directory: Option<PathBuf>,
    state: Mutex<State>,
    pub wake: tokio::sync::Notify,
    limit: u64,
    session_id: Uuid,
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
        let settings = directory.as_ref().map(|p| p.join("budgets.json"));
        let loaded = settings.as_ref().filter(|p| p.exists()).map(|p| {
            if fs::metadata(p).map_or(true, |m| m.len() > 65536) {
                return Err(());
            }
            let settings: Vec<BudgetSetting> =
                serde_json::from_slice(&fs::read(p).map_err(|_| ())?).map_err(|_| ())?;
            if settings.len() > 256 || settings.iter().any(|b| !valid_budget(b)) {
                return Err(());
            }
            Ok(settings)
        });
        let (budget_settings, error) = match loaded {
            Some(Ok(s)) => (s, None),
            Some(Err(())) => (
                vec![],
                Some("Не удалось прочитать локальные бюджеты памяти.".into()),
            ),
            None => (vec![], None),
        };
        Self {
            directory,
            state: Mutex::new(State {
                enabled,
                error,
                last: None,
                export_path: None,
                budget_settings,
                tracker: Default::default(),
                interrupted: true,
            }),
            wake: Default::default(),
            limit,
            session_id: Uuid::new_v4(),
        }
    }
    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }
    #[cfg(test)]
    pub fn action(&self, action: Action) -> Result<Status, &'static str> {
        self.request(action, None)
    }
    pub fn request(
        &self,
        action: Action,
        budget: Option<BudgetSetting>,
    ) -> Result<Status, &'static str> {
        let mut state = self.state.lock().unwrap();
        match action {
            Action::SetBudget => {
                let budget = budget
                    .filter(valid_budget)
                    .ok_or("Некорректный бюджет памяти (от 1 МиБ до 1 ПиБ).")?;
                let mut settings = state.budget_settings.clone();
                if let Some(old) = settings.iter_mut().find(|b| {
                    b.environment_id == budget.environment_id && b.worktree_id == budget.worktree_id
                }) {
                    *old = budget.clone();
                } else if settings.len() < 256 {
                    settings.push(budget.clone());
                } else {
                    return Err("Слишком много настроек бюджета (максимум 256).");
                }
                let directory = self
                    .directory
                    .as_ref()
                    .ok_or("Каталог диагностики недоступен.")?;
                fs::create_dir_all(directory).map_err(|_| "Каталог диагностики недоступен.")?;
                let bytes =
                    serde_json::to_vec(&settings).map_err(|_| "Некорректный бюджет памяти.")?;
                agent_workspace_backend::storage::atomic_write(
                    &directory.join("budgets.json"),
                    &bytes,
                )
                .map_err(|_| "Не удалось сохранить локальный бюджет памяти.")?;
                state.tracker.reset_budget(&budget);
                state.budget_settings = settings;
                // Re-evaluate on the existing 30-second collector cycle.
            }
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
                state.interrupted = true;
            }
            Action::ExportSummary => {
                let directory = self
                    .directory
                    .as_ref()
                    .ok_or("Каталог диагностики недоступен.")?;
                fs::create_dir_all(directory).map_err(|_| "Каталог диагностики недоступен.")?;
                let path = directory.join(format!(
                    "memory-summary-{}-{}.json",
                    unix_ms(),
                    Uuid::new_v4()
                ));
                summary::export(directory, &path, unix_ms())
                    .map_err(|_| "Не удалось экспортировать сводку диагностики.")?;
                state.export_path = Some(path.to_string_lossy().into_owned());
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
                            "record_type":"metadata", "schema_version":2, "application_version":env!("CARGO_PKG_VERSION"),
                            "exported_unix_ms":unix_ms(), "interval_seconds":INTERVAL_SECONDS,
                            "max_retained_bytes":self.limit*2,
                            "budget_settings":state.budget_settings,
                            "budgets":tracking::statuses(&state.budget_settings,state.last.as_ref(),state.enabled),
                            "semantics": "All memory in bytes. Linux private_bytes=USS; Windows private_bytes=private commit, not resident RAM. Worktree accounted_bytes sums each observed owned process once: PSS if available, otherwise RSS (conservative shared-page overcount); missing both and incomplete inventory mean unknown, never confirmed below budget. RSS/PSs exclude swap. Missing metrics have counts; sums are available subtotals. Full lightweight groups precede detail truncation; expensive PSS is limited to 128 processes. Windows inventory excludes unrelated applications. Never sum Windows Vmmem and Linux processes. Linux meminfo/PSI may span WSL2 distros. CPU percent is elapsed CPU / wall time, can exceed 100 on multiple cores. Deltas reset after gaps >90s or source/run changes. Lifetime strings preserve exact start IDs; numeric Windows start IDs may exceed JS safe integers. Peaks are observations in the current app session, not simultaneous maxima. Events describe bounded observation, never prove process exit. Worktree ownership uses live PTY ancestry, not cwd. Shared services, detached/orphaned processes and externally started containers may be outside budgets. Container detection and name/Java roles are heuristics. PSI averages are stall percentages, total_us is cumulative stall microseconds.",
                            "privacy": "No full commands, arguments, environment, secrets, project paths, terminal text or prompts are stored. Linux Java only: read at most 16KiB argv locally and discard it after mapping to a fixed role (test_worker, gradle_daemon, build_tool, language_server, compiler_daemon, unknown_jvm). Cgroup text is inspected locally for a boolean container hint and discarded. Export includes process basenames, IDs, boot/run identity, numeric metrics and fixed classifications. Budgets are local, disabled by default, warning-only; no OS limits or process control.",
                            "compatibility":"Existing schema-1 JSONL lines are streamed unchanged alongside schema-2 lines; missing new fields mean unavailable, not zero.",
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
            budget_settings: state.budget_settings.clone(),
            budgets: tracking::statuses(&state.budget_settings, state.last.as_ref(), state.enabled),
        }
    }
    pub fn append(&self, mut record: Record) -> Vec<BudgetStatus> {
        let mut state = self.state.lock().unwrap();
        if !state.enabled {
            return vec![];
        }
        record.schema_version = 2;
        record.session_id = self.session_id;
        let previous = state.last.take();
        let settings = state.budget_settings.clone();
        let interrupted = state.interrupted;
        state.tracker.update(
            &mut record,
            previous.as_ref().filter(|_| !interrupted),
            &settings,
        );
        if interrupted && previous.is_some() {
            for id in record
                .host
                .iter()
                .map(|_| record.host_environment_id)
                .chain(
                    record
                        .environments
                        .iter()
                        .filter(|e| e.snapshot.is_some())
                        .map(|e| e.environment_id),
                )
            {
                if record.events.len() == 1024 {
                    record.events.pop();
                    record.omitted_events += 1;
                }
                record.events.push(Event {
                    kind: "observation_gap".into(),
                    environment_id: id,
                    worktree_id: None,
                    previous_worktree_id: None,
                    lifetime_id: None,
                    reason: "recorder_paused".into(),
                });
            }
        }
        state.interrupted = false;
        // Fixed point including the size field itself, without retaining JSONL in RAM.
        for _ in 0..3 {
            record.snapshot_bytes = serde_json::to_vec(&record).map_or(0, |b| b.len() + 1);
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
        let budgets = record.budgets.clone();
        state.last = Some(record);
        budgets
    }
}

fn valid_budget(b: &BudgetSetting) -> bool {
    !b.environment_id.is_nil()
        && !b.worktree_id.is_nil()
        && b.limit_bytes
            .is_none_or(|n| (1024 * 1024..=1u64 << 50).contains(&n))
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
    fn summary_export_keeps_recording_and_original_history() {
        let directory =
            std::env::temp_dir().join(format!("branchline-summary-action-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), 16384);
        recorder.action(Action::Start).unwrap();
        recorder.append(Record {
            record_type: "sample",
            captured_unix_ms: 1000,
            ..Default::default()
        });
        let before = recorder.action(Action::Status).unwrap().retained_bytes;
        let status = recorder.action(Action::ExportSummary).unwrap();
        assert!(status.enabled);
        assert_eq!(status.retained_bytes, before);
        let summary: serde_json::Value =
            serde_json::from_slice(&fs::read(status.export_path.unwrap()).unwrap()).unwrap();
        assert_eq!(summary["record_type"], "memory_summary");
        assert_eq!(summary["samples"], 1);
        let raw = recorder.action(Action::Export).unwrap();
        assert!(raw.export_path.unwrap().ends_with(".jsonl"));
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn measures_native_record_cost_without_printing_process_details() {
        let directory =
            std::env::temp_dir().join(format!("branchline-memory-cost-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), LOG_LIMIT);
        recorder.action(Action::Start).unwrap();
        let start = std::time::Instant::now();
        let snapshot = agent_workspace_backend::memory::sample(&Default::default());
        let collection_duration_ms = start.elapsed().as_millis() as u64;
        let platform = snapshot.platform.clone();
        recorder.append(Record {
            record_type: "sample",
            captured_unix_ms: unix_ms(),
            collection_duration_ms,
            host: Some(snapshot),
            ..Default::default()
        });
        let sample = recorder
            .action(Action::Status)
            .unwrap()
            .last_sample
            .unwrap();
        println!("memory record benchmark: platform={platform} collection_ms={collection_duration_ms} collect_track_write_ms={} record_bytes={} events={}",start.elapsed().as_millis(),sample.snapshot_bytes,sample.events.len());
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn budgets_are_local_opt_in_persist_disabled_and_export_legacy_lines_unchanged() {
        let directory =
            std::env::temp_dir().join(format!("branchline-memory-budget-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), 16384);
        let b = BudgetSetting {
            environment_id: Uuid::new_v4(),
            worktree_id: Uuid::new_v4(),
            limit_bytes: Some(1024 * 1024 * 1024),
        };
        let status = recorder
            .request(Action::SetBudget, Some(b.clone()))
            .unwrap();
        assert!(!status.enabled);
        assert_eq!(status.budgets[0].state, "diagnostics_disabled");
        assert_eq!(
            Recorder::at(Some(directory.clone()), 16384)
                .action(Action::Status)
                .unwrap()
                .budget_settings,
            vec![b.clone()]
        );
        let off = BudgetSetting {
            limit_bytes: None,
            ..b
        };
        recorder
            .request(Action::SetBudget, Some(off.clone()))
            .unwrap();
        assert_eq!(
            Recorder::at(Some(directory.clone()), 16384)
                .action(Action::Status)
                .unwrap()
                .budget_settings,
            vec![off]
        );
        let legacy = "{\"record_type\":\"sample\",\"schema_version\":1,\"errors\":[]}\n";
        fs::write(directory.join("memory-current.jsonl"), legacy).unwrap();
        recorder.action(Action::Start).unwrap();
        recorder.append(Record {
            record_type: "sample",
            captured_unix_ms: unix_ms(),
            ..Default::default()
        });
        let status = recorder.action(Action::Export).unwrap();
        let output = fs::read_to_string(status.export_path.unwrap()).unwrap();
        assert!(output.contains(legacy));
        let lines: Vec<serde_json::Value> = output
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(lines[0]["schema_version"], 2);
        assert_eq!(lines[1]["schema_version"], 1);
        assert_eq!(lines[2]["schema_version"], 2);
        assert!(lines[0].get("budget_settings").is_some());
        assert_eq!(
            status.last_sample.as_ref().unwrap().snapshot_bytes,
            serde_json::to_vec(status.last_sample.as_ref().unwrap())
                .unwrap()
                .len()
                + 1
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn invalid_settings_are_rejected_without_changing_saved_budget() {
        let directory =
            std::env::temp_dir().join(format!("branchline-invalid-budget-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), 16384);
        let b = BudgetSetting {
            environment_id: Uuid::nil(),
            worktree_id: Uuid::new_v4(),
            limit_bytes: Some(1),
        };
        assert!(recorder.request(Action::SetBudget, Some(b)).is_err());
        assert!(!directory.exists());
    }
    #[test]
    fn pausing_recorder_breaks_deltas_even_for_a_short_gap() {
        let directory =
            std::env::temp_dir().join(format!("branchline-memory-pause-{}", Uuid::new_v4()));
        let recorder = Recorder::at(Some(directory.clone()), 16384);
        let make = |time, bytes| Record {
            record_type: "sample",
            captured_unix_ms: time,
            host: Some(Snapshot {
                source_id: "same-run".into(),
                processes: vec![agent_workspace_backend::memory::ProcessMemory {
                    lifetime_id: Some("same-run:1:1".into()),
                    resident_bytes: Some(bytes),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        recorder.action(Action::Start).unwrap();
        recorder.append(make(1000, 10));
        recorder.action(Action::Stop).unwrap();
        recorder.action(Action::Start).unwrap();
        recorder.append(make(2000, 20));
        let status = recorder.action(Action::Status).unwrap();
        let sample = status.last_sample.unwrap();
        assert!(sample.host.unwrap().processes[0]
            .resident_delta_bytes
            .is_none());
        assert!(sample
            .events
            .iter()
            .any(|e| e.kind == "observation_gap" && e.reason == "recorder_paused"));
        fs::remove_dir_all(directory).unwrap();
    }
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
            ..Default::default()
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
