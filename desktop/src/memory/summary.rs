//! Streaming, bounded export for analysis; the original JSONL remains available.
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
};

const MAX_LINE: usize = 4 * 1024 * 1024;
const MAX_SOURCES: usize = 64;
const MAX_SESSIONS: usize = 128;
const MAX_CATEGORIES: usize = 32;

#[derive(Default, Serialize)]
struct Range {
    observations: u64,
    min: u64,
    max: u64,
    min_at_unix_ms: u64,
    max_at_unix_ms: u64,
}
impl Range {
    fn add(&mut self, value: u64, at: u64) {
        if self.observations == 0 || value < self.min {
            self.min = value;
            self.min_at_unix_ms = at;
        }
        if self.observations == 0 || value > self.max {
            self.max = value;
            self.max_at_unix_ms = at;
        }
        self.observations += 1;
    }
}

#[derive(Default, Serialize)]
struct Category {
    accounted_bytes: Range,
    process_count: Range,
    observations_using_rss_fallback: u64,
    observations_missing_accounting: u64,
}

#[derive(Default, Serialize)]
struct Source {
    observations: u64,
    unavailable_observations: u64,
    incomplete_inventory_observations: u64,
    unknown_inventory_observations: u64,
    truncated_detail_observations: u64,
    category_observations_from_full_groups: u64,
    category_observations_from_details_only: u64,
    windows_available_below_1gib_observations: u64,
    system: BTreeMap<String, Range>,
    categories: BTreeMap<String, Category>,
    min_available_snapshot: Option<Value>,
}

#[derive(Default, Serialize)]
struct Session {
    samples: u64,
    first_unix_ms: u64,
    last_unix_ms: u64,
}

#[derive(Default, Serialize)]
struct Summary {
    record_type: &'static str,
    schema_version: u8,
    application_version: &'static str,
    exported_unix_ms: u64,
    samples: u64,
    invalid_lines: u64,
    oversized_lines: u64,
    bounded_entries_omitted: u64,
    schema_versions: BTreeMap<String, u64>,
    sessions: BTreeMap<String, Session>,
    sources: BTreeMap<String, Source>,
    semantics: Vec<&'static str>,
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key)?.as_u64()
}
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .chars()
        .take(160)
        .collect()
}
fn rows<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

impl Summary {
    fn new(at: u64) -> Self {
        Self {
            record_type: "memory_summary",
            schema_version: 1,
            application_version: env!("CARGO_PKG_VERSION"),
            exported_unix_ms: at,
            semantics: vec![
                "Memory is in bytes. Ranges are observed per metric; peaks at different times must not be added.",
                "Never add Windows vmmem to Linux memory. Windows private commit is not physical RAM.",
                "Category accounting is PSS per process, otherwise RSS; shared pages may be counted repeatedly.",
                "Missing values are unknown, not zero. Categories are conditional on presence; observations=0 means unavailable.",
                "Old records without full groups use bounded process details only; these are partial subtotals.",
                "Sources are keyed by environment, not boot. Min/max do not imply continuity across restarts or log gaps.",
                "Sample counts are not elapsed time. Session endpoints do not prove idle, process exit or leaks.",
                "Windows inventory covers app/owned processes and WSL, not all Windows applications.",
                "Roles and container hints are heuristics. No full commands, arguments, environment, paths or terminal contents are exported.",
                "Limits: 64 sources, 128 app sessions, 32 categories per source, 4 MiB per input line. Loss is counted explicitly.",
            ],
            ..Default::default()
        }
    }

    fn read(&mut self, input: impl io::Read) -> io::Result<()> {
        let mut reader = BufReader::new(input);
        let mut line = Vec::new();
        loop {
            line.clear();
            // A corrupt/oversized line cannot allocate the entire log in RAM.
            let count = reader
                .by_ref()
                .take((MAX_LINE + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            if line.len() > MAX_LINE {
                self.oversized_lines += 1;
                if line.last() != Some(&b'\n') {
                    loop {
                        let bytes = reader.fill_buf()?;
                        if bytes.is_empty() {
                            break;
                        }
                        let newline = bytes.iter().position(|b| *b == b'\n');
                        let used = newline.map_or(bytes.len(), |n| n + 1);
                        reader.consume(used);
                        if newline.is_some() {
                            break;
                        }
                    }
                }
                continue;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(value) if value.get("record_type").and_then(Value::as_str) == Some("sample") => {
                    self.sample(&value);
                }
                Ok(_) => {}
                Err(_) => self.invalid_lines += 1,
            }
        }
        Ok(())
    }

    fn sample(&mut self, record: &Value) {
        let Some(at) = number(record, "captured_unix_ms") else {
            self.invalid_lines += 1;
            return;
        };
        self.samples += 1;
        let version = number(record, "schema_version").unwrap_or(0).to_string();
        if self.schema_versions.contains_key(&version) || self.schema_versions.len() < 16 {
            *self.schema_versions.entry(version).or_default() += 1;
        } else {
            self.bounded_entries_omitted += 1;
        }
        let id = text(record, "session_id");
        if self.sessions.contains_key(&id) || self.sessions.len() < MAX_SESSIONS {
            let session = self.sessions.entry(id).or_default();
            if session.samples == 0 {
                session.first_unix_ms = at;
            }
            session.first_unix_ms = session.first_unix_ms.min(at);
            session.last_unix_ms = session.last_unix_ms.max(at);
            session.samples += 1;
        } else {
            self.bounded_entries_omitted += 1;
        }
        self.source(
            format!("windows:{}", text(record, "host_environment_id")),
            record.get("host"),
            at,
        );
        for env in rows(record, "environments") {
            self.source(
                format!("linux:{}", text(env, "environment_id")),
                env.get("snapshot"),
                at,
            );
        }
    }

    fn source(&mut self, id: String, snapshot: Option<&Value>, at: u64) {
        if !self.sources.contains_key(&id) && self.sources.len() >= MAX_SOURCES {
            self.bounded_entries_omitted += 1;
            return;
        }
        let source = self.sources.entry(id).or_default();
        let Some(snapshot) = snapshot.filter(|s| s.is_object()) else {
            source.unavailable_observations += 1;
            return;
        };
        source.observations += 1;
        match snapshot
            .pointer("/coverage/inventory_complete")
            .and_then(Value::as_bool)
        {
            Some(false) => source.incomplete_inventory_observations += 1,
            None => source.unknown_inventory_observations += 1,
            _ => {}
        }
        source.truncated_detail_observations +=
            u64::from(number(snapshot, "omitted_processes").unwrap_or(0) > 0);
        if let Some(system) = snapshot.get("system") {
            for key in [
                "total_bytes",
                "available_bytes",
                "free_bytes",
                "cached_bytes",
                "reclaimable_slab_bytes",
                "unreclaimable_slab_bytes",
                "anonymous_bytes",
                "shmem_bytes",
                "swap_total_bytes",
                "swap_free_bytes",
            ] {
                if let Some(value) = number(system, key) {
                    let range = source.system.entry(key.into()).or_default();
                    if key == "available_bytes" && (range.observations == 0 || value < range.min) {
                        source.min_available_snapshot = Some(evidence(snapshot, at));
                    }
                    range.add(value, at);
                }
            }
            if let (Some(total), Some(free)) = (
                number(system, "swap_total_bytes"),
                number(system, "swap_free_bytes"),
            ) {
                if let Some(used) = total.checked_sub(free) {
                    source
                        .system
                        .entry("swap_used_bytes".into())
                        .or_default()
                        .add(used, at);
                }
            }
            if snapshot.get("platform").and_then(Value::as_str) == Some("windows") {
                source.windows_available_below_1gib_observations += u64::from(
                    number(system, "available_bytes").is_some_and(|v| v < 1024 * 1024 * 1024),
                );
            }
        }
        // Aggregate categories across all groups in this single snapshot before observing peaks.
        let mut totals: BTreeMap<String, [u64; 4]> = BTreeMap::new();
        let full_groups = rows(snapshot, "groups").next().is_some();
        if full_groups {
            source.category_observations_from_full_groups += 1;
            for group in rows(snapshot, "groups") {
                for category in rows(group, "categories") {
                    let key = text(category, "category");
                    if !totals.contains_key(&key) && totals.len() >= MAX_CATEGORIES {
                        self.bounded_entries_omitted += 1;
                        continue;
                    }
                    let Some(t) = category.get("totals") else {
                        continue;
                    };
                    let entry = totals.entry(key).or_default();
                    let count = number(t, "processes").unwrap_or(0);
                    let accounted = number(t, "accounted_bytes");
                    entry[0] = entry[0].saturating_add(accounted.unwrap_or(0));
                    entry[1] = entry[1].saturating_add(count);
                    entry[2] =
                        entry[2].saturating_add(number(t, "missing_pss").unwrap_or(count.max(1)));
                    let missing = if accounted.is_none() {
                        count.max(1)
                    } else {
                        number(t, "missing_accounting").unwrap_or(count.max(1))
                    };
                    entry[3] = entry[3].saturating_add(missing);
                }
            }
        } else {
            source.category_observations_from_details_only += 1;
            for process in rows(snapshot, "processes") {
                let key = text(process, "category");
                if !totals.contains_key(&key) && totals.len() >= MAX_CATEGORIES {
                    self.bounded_entries_omitted += 1;
                    continue;
                }
                let entry = totals.entry(key).or_default();
                let value =
                    number(process, "pss_bytes").or_else(|| number(process, "resident_bytes"));
                entry[0] = entry[0].saturating_add(value.unwrap_or(0));
                entry[1] += 1;
                entry[2] += u64::from(number(process, "pss_bytes").is_none());
                entry[3] += u64::from(value.is_none());
            }
        }
        for (key, total) in totals {
            if !source.categories.contains_key(&key) && source.categories.len() >= MAX_CATEGORIES {
                self.bounded_entries_omitted += 1;
                continue;
            }
            let category = source.categories.entry(key).or_default();
            // A wholly unavailable subtotal is not an observed zero.
            if total[0] > 0 || total[1] > total[3] {
                category.accounted_bytes.add(total[0], at);
            }
            category.process_count.add(total[1], at);
            category.observations_using_rss_fallback += u64::from(total[2] > 0);
            category.observations_missing_accounting += u64::from(total[3] > 0);
        }
    }
}

fn evidence(snapshot: &Value, at: u64) -> Value {
    let mut processes: Vec<_> = rows(snapshot, "processes").collect();
    processes.sort_by_key(|p| {
        std::cmp::Reverse(
            number(p, "pss_bytes")
                .or_else(|| number(p, "resident_bytes"))
                .unwrap_or(0),
        )
    });
    let top: Vec<_> = processes
        .into_iter()
        .take(8)
        .map(|p| {
            serde_json::json!({
                "name":text(p,"name"), "category":text(p,"category"), "role":text(p,"role"),
                "pid":number(p,"pid"), "lifetime_id":text(p,"lifetime_id"),
                "resident_bytes":number(p,"resident_bytes"), "pss_bytes":number(p,"pss_bytes"),
                "worktree_id":text(p,"worktree_id")
            })
        })
        .collect();
    serde_json::json!({"captured_unix_ms":at,"source_id":text(snapshot,"source_id"),"top_processes":top})
}

pub(super) fn export(directory: &Path, destination: &Path, at: u64) -> io::Result<()> {
    let mut summary = Summary::new(at);
    for name in ["memory-previous.jsonl", "memory-current.jsonl"] {
        match File::open(directory.join(name)) {
            Ok(input) => summary.read(input)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut output, &summary)?;
        output.write_all(b"\n")?;
        output.sync_all()
    })();
    drop(output);
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: u64, value: Value) -> Value {
        serde_json::json!({"record_type":"sample","schema_version":2,"captured_unix_ms":at,
            "session_id":"session","host_environment_id":"host","host":value})
    }
    fn read(summary: &mut Summary, value: Value) {
        summary.read(format!("{value}\n").as_bytes()).unwrap();
    }
    #[test]
    fn peaks_are_simultaneous_group_sums_and_missing_memory_is_not_zero() {
        let mut summary = Summary::new(1);
        let group = |value| {
            serde_json::json!({"categories":[{"category":"java","totals":{
            "processes":1,"accounted_bytes":value,"missing_pss":0,"missing_accounting":0}}]})
        };
        read(
            &mut summary,
            sample(
                100,
                serde_json::json!({"system":{"available_bytes":20},"groups":[group(10),group(30)]}),
            ),
        );
        read(
            &mut summary,
            sample(
                200,
                serde_json::json!({"system":{"available_bytes":null},"groups":[group(30),group(10)]}),
            ),
        );
        read(
            &mut summary,
            sample(
                300,
                serde_json::json!({"system":{"available_bytes":10},"groups":[group(5),group(15)]}),
            ),
        );
        let source = &summary.sources["windows:host"];
        assert_eq!(source.system["available_bytes"].observations, 2);
        assert_eq!(source.system["available_bytes"].min_at_unix_ms, 300);
        assert_eq!(source.categories["java"].accounted_bytes.max, 40);
        assert_eq!(
            source.categories["java"].accounted_bytes.max_at_unix_ms,
            100
        );
        assert_eq!(source.categories["java"].process_count.max, 2);
    }
    #[test]
    fn mixed_schema_unavailable_sources_and_corrupt_lines_are_explicit() {
        let mut summary = Summary::new(1);
        summary
            .read(b"{broken}\n{\"record_type\":\"metadata\"}\n".as_slice())
            .unwrap();
        read(
            &mut summary,
            serde_json::json!({"record_type":"sample","schema_version":1,"captured_unix_ms":10,
            "host":null,"environments":[{"environment_id":"env","snapshot":null}]}),
        );
        read(
            &mut summary,
            sample(
                20,
                serde_json::json!({"processes":[
            {"category":"java","resident_bytes":100,"pss_bytes":null},
            {"category":"other","resident_bytes":null,"pss_bytes":null}]}),
            ),
        );
        assert_eq!(summary.samples, 2);
        assert_eq!(summary.invalid_lines, 1);
        assert_eq!(summary.sources["linux:env"].unavailable_observations, 1);
        let source = &summary.sources["windows:host"];
        assert_eq!(source.category_observations_from_details_only, 1);
        assert_eq!(source.categories["java"].accounted_bytes.max, 100);
        assert_eq!(source.categories["java"].observations_using_rss_fallback, 1);
        assert_eq!(source.categories["other"].accounted_bytes.observations, 0);
    }
    #[test]
    fn oversized_lines_are_skipped_without_losing_the_following_sample() {
        let mut input = vec![b'x'; MAX_LINE + 100];
        input.push(b'\n');
        input.extend_from_slice(format!("{}\n", sample(1, Value::Null)).as_bytes());
        let mut summary = Summary::new(1);
        summary.read(input.as_slice()).unwrap();
        assert_eq!(summary.oversized_lines, 1);
        assert_eq!(summary.samples, 1);
    }
    #[test]
    fn identity_growth_is_bounded_and_loss_is_reported() {
        let mut summary = Summary::new(1);
        for n in 0..MAX_SESSIONS + 10 {
            let mut record = sample(n as u64, Value::Null);
            record["session_id"] = Value::String(format!("session-{n}"));
            record["host_environment_id"] = Value::String(format!("host-{n}"));
            read(&mut summary, record);
        }
        assert_eq!(summary.sessions.len(), MAX_SESSIONS);
        assert_eq!(summary.sources.len(), MAX_SOURCES);
        assert!(summary.bounded_entries_omitted > 0);
    }
    #[test]
    fn export_preserves_raw_logs_and_rejects_destination_collisions() {
        let dir = std::env::temp_dir().join(format!("branchline-summary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let raw = format!(
            "{}\n",
            sample(10, serde_json::json!({"system":{"available_bytes":100}}))
        );
        std::fs::write(dir.join("memory-current.jsonl"), &raw).unwrap();
        let destination = dir.join("summary.json");
        export(&dir, &destination, 100).unwrap();
        let output: Value = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
        assert_eq!(output["samples"], 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("memory-current.jsonl")).unwrap(),
            raw
        );
        assert!(export(&dir, &destination, 200).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(destination).unwrap()).unwrap()
                ["exported_unix_ms"],
            100
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
