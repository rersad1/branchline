use super::*;
use agent_workspace_backend::memory::GroupSummary;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BudgetSetting {
    pub environment_id: Uuid,
    pub worktree_id: Uuid,
    /// None disables this worktree's budget. Binary units, persisted locally.
    pub limit_bytes: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
pub struct BudgetStatus {
    #[serde(flatten)]
    pub setting: BudgetSetting,
    pub state: String,
    pub accounted_bytes: Option<u64>,
    pub excess_bytes: Option<u64>,
    pub peak_accounted_bytes: u64,
    pub complete: bool,
    pub missing_pss: usize,
    pub missing_accounting: usize,
    pub fallback_rss: bool,
    pub warning_active: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub kind: String,
    pub environment_id: Uuid,
    pub worktree_id: Option<Uuid>,
    pub previous_worktree_id: Option<Uuid>,
    pub lifetime_id: Option<String>,
    pub reason: String,
}
impl Event {
    fn new(kind: &str, environment: Uuid, worktree: Option<Uuid>, reason: &str) -> Self {
        Self {
            kind: kind.into(),
            environment_id: environment,
            worktree_id: worktree,
            previous_worktree_id: None,
            lifetime_id: None,
            reason: reason.into(),
        }
    }
}
#[derive(Default)]
pub struct Tracker {
    group_peaks: HashMap<(Uuid, String, Option<Uuid>), u64>,
    process_peaks: HashMap<String, (Option<u64>, Option<u64>)>,
    over: HashSet<(Uuid, Uuid)>,
}
fn snapshots(record: &Record) -> Vec<(Uuid, &Snapshot)> {
    record
        .host
        .iter()
        .map(|s| (record.host_environment_id, s))
        .chain(
            record
                .environments
                .iter()
                .filter_map(|e| e.snapshot.as_ref().map(|s| (e.environment_id, s))),
        )
        .collect()
}
fn delta(a: Option<u64>, b: Option<u64>) -> Option<i64> {
    a.zip(b).map(|(a, b)| {
        (i128::from(a) - i128::from(b)).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    })
}
fn peak(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a.into_iter().chain(b).max()
}
impl Tracker {
    pub fn reset_budget(&mut self, setting: &BudgetSetting) {
        self.over
            .remove(&(setting.environment_id, setting.worktree_id));
    }
    pub fn update(
        &mut self,
        record: &mut Record,
        previous: Option<&Record>,
        settings: &[BudgetSetting],
    ) {
        let elapsed =
            previous.and_then(|p| record.captured_unix_ms.checked_sub(p.captured_unix_ms));
        let gap =
            previous.is_some() && elapsed.is_none_or(|ms| ms > INTERVAL_SECONDS * 3000 || ms == 0);
        let old: HashMap<_, _> = previous
            .map(snapshots)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut events = vec![];
        let now_ids: HashSet<_> = snapshots(record).iter().map(|(id, _)| *id).collect();
        for id in old.keys().filter(|id| !now_ids.contains(id)) {
            events.push(Event::new(
                "environment_unavailable",
                *id,
                None,
                "observation_interrupted_not_process_exit",
            ));
        }
        let host_id = record.host_environment_id;
        let mut current: Vec<_> = record.host.iter_mut().map(|s| (host_id, s)).collect();
        current.extend(
            record
                .environments
                .iter_mut()
                .filter_map(|e| e.snapshot.as_mut().map(|s| (e.environment_id, s))),
        );
        for (id, snapshot) in current {
            let prior = old.get(&id).copied();
            let same_source = prior.is_some_and(|p| {
                !snapshot.source_id.is_empty() && p.source_id == snapshot.source_id
            });
            if prior.is_some() && !same_source {
                events.push(Event::new(
                    "source_changed",
                    id,
                    None,
                    "new_boot_or_collector_run",
                ));
            }
            if gap {
                events.push(Event::new(
                    "observation_gap",
                    id,
                    None,
                    "deltas_unavailable",
                ));
            }
            let continuous = same_source && !gap;
            let old_processes: HashMap<_, _> = prior
                .filter(|_| same_source)
                .into_iter()
                .flat_map(|s| &s.processes)
                .filter_map(|p| p.lifetime_id.as_ref().map(|key| (key, p)))
                .collect();
            for p in &mut snapshot.processes {
                let Some(key) = &p.lifetime_id else {
                    continue;
                };
                let old_p = old_processes.get(key).copied();
                if let Some(old_p) = old_p {
                    if old_p.worktree_id != p.worktree_id
                        || old_p.session_id != p.session_id
                        || old_p.relation != p.relation
                    {
                        let mut event = Event::new(
                            "owner_changed",
                            id,
                            p.worktree_id,
                            "observed_ancestry_changed",
                        );
                        event.previous_worktree_id = old_p.worktree_id;
                        event.lifetime_id = Some(key.clone());
                        events.push(event);
                    }
                    if continuous {
                        p.resident_delta_bytes = delta(p.resident_bytes, old_p.resident_bytes);
                        p.pss_delta_bytes = delta(p.pss_bytes, old_p.pss_bytes);
                        p.cpu_delta_ms = p
                            .cpu_time_ms
                            .zip(old_p.cpu_time_ms)
                            .and_then(|(a, b)| a.checked_sub(b));
                        p.cpu_percent = p
                            .cpu_delta_ms
                            .zip(elapsed)
                            .filter(|(_, ms)| *ms > 0)
                            .map(|(cpu, ms)| cpu as f64 / ms as f64 * 100.0);
                    }
                } else {
                    let mut event = Event::new(
                        "process_observed",
                        id,
                        p.worktree_id,
                        "entered_bounded_detail_sample",
                    );
                    event.lifetime_id = Some(key.clone());
                    events.push(event);
                }
                let peaks = self.process_peaks.get(key).copied().unwrap_or_default();
                p.peak_resident_bytes = peak(peaks.0, p.resident_bytes);
                p.peak_pss_bytes = peak(peaks.1, p.pss_bytes);
                if self.process_peaks.len() < 2048 || self.process_peaks.contains_key(key) {
                    self.process_peaks
                        .insert(key.clone(), (p.peak_resident_bytes, p.peak_pss_bytes));
                } else {
                    record.errors.push("process_peak_history_capacity".into());
                }
            }
            let keys: HashSet<_> = snapshot
                .processes
                .iter()
                .filter_map(|p| p.lifetime_id.as_ref())
                .collect();
            for (key, p) in old_processes.iter().filter(|(key, _)| !keys.contains(*key)) {
                let mut event = Event::new(
                    "process_unobserved",
                    id,
                    p.worktree_id,
                    "absent_from_bounded_sample_not_proof_of_exit",
                );
                event.lifetime_id = Some((*key).clone());
                events.push(event);
            }
            for g in &mut snapshot.groups {
                let key = (id, g.scope.clone(), g.worktree_id);
                let old_peak = self.group_peaks.get(&key).copied().unwrap_or(0);
                g.peak_accounted_bytes = old_peak.max(g.totals.accounted_bytes);
                if self.group_peaks.len() < 512 || self.group_peaks.contains_key(&key) {
                    self.group_peaks.insert(key, g.peak_accounted_bytes);
                } else {
                    record.errors.push("group_peak_history_capacity".into());
                }
                if continuous {
                    g.accounted_delta_bytes = prior
                        .and_then(|s| {
                            s.groups.iter().find(|old| {
                                old.scope == g.scope && old.worktree_id == g.worktree_id
                            })
                        })
                        .and_then(|old| {
                            delta(
                                Some(g.totals.accounted_bytes),
                                Some(old.totals.accounted_bytes),
                            )
                        });
                }
            }
        }
        record.budget_settings = settings.to_vec();
        let sources: HashMap<_, _> = snapshots(record).into_iter().collect();
        record.budgets = settings
            .iter()
            .map(|setting| {
                let snapshot = sources.get(&setting.environment_id).copied();
                let group = snapshot.and_then(|s| {
                    s.groups.iter().find(|g| {
                        g.scope == "worktree" && g.worktree_id == Some(setting.worktree_id)
                    })
                });
                let complete = snapshot.is_some_and(|s| s.coverage.inventory_complete)
                    && group.is_some_and(|g| g.totals.missing_accounting == 0);
                let mut status = budget_status(setting, group, complete);
                status.peak_accounted_bytes = self
                    .group_peaks
                    .get(&(
                        setting.environment_id,
                        "worktree".into(),
                        Some(setting.worktree_id),
                    ))
                    .copied()
                    .unwrap_or(status.peak_accounted_bytes);
                let key = (setting.environment_id, setting.worktree_id);
                if let (Some(limit), Some(used)) = (setting.limit_bytes, status.accounted_bytes) {
                    if used > limit && self.over.insert(key) {
                        events.push(Event::new(
                            "budget_exceeded",
                            setting.environment_id,
                            Some(setting.worktree_id),
                            "soft_warning_only",
                        ));
                    } else if complete
                        && used <= limit.saturating_mul(9) / 10
                        && self.over.remove(&key)
                    {
                        events.push(Event::new(
                            "budget_recovered",
                            setting.environment_id,
                            Some(setting.worktree_id),
                            "below_90_percent",
                        ));
                    }
                    status.warning_active = self.over.contains(&key);
                    if status.warning_active {
                        status.state = if used > limit {
                            "exceeded"
                        } else {
                            "hysteresis"
                        }
                        .into();
                    }
                }
                status
            })
            .collect();
        // A busy first observation must never push a budget warning out of the log.
        events.sort_by_key(|e| {
            if e.kind.starts_with("budget_") {
                0
            } else if e.kind.starts_with("process_") {
                2
            } else {
                1
            }
        });
        record.omitted_events = events.len().saturating_sub(1024);
        events.truncate(1024);
        record.events = events;
        record.errors.sort();
        record.errors.dedup();
    }
}
fn budget_status(
    setting: &BudgetSetting,
    group: Option<&GroupSummary>,
    complete: bool,
) -> BudgetStatus {
    let used = group.map(|g| g.totals.accounted_bytes);
    BudgetStatus {
        setting: setting.clone(),
        state: if setting.limit_bytes.is_none() {
            "disabled"
        } else if group.is_none() {
            "unavailable_or_unattributed"
        } else if !complete {
            "incomplete"
        } else {
            "observed_below_budget"
        }
        .into(),
        accounted_bytes: used,
        excess_bytes: used
            .zip(setting.limit_bytes)
            .map(|(a, b)| a.saturating_sub(b)),
        peak_accounted_bytes: group.map_or(0, |g| g.peak_accounted_bytes),
        complete,
        missing_pss: group.map_or(0, |g| g.totals.missing_pss),
        missing_accounting: group.map_or(0, |g| g.totals.missing_accounting),
        fallback_rss: group.is_some_and(|g| g.totals.missing_pss > 0),
        warning_active: false,
    }
}

// Status reads use the existing sample; they never trigger a collection.
pub fn statuses(
    settings: &[BudgetSetting],
    record: Option<&Record>,
    enabled: bool,
) -> Vec<BudgetStatus> {
    settings
        .iter()
        .map(|setting| {
            let mut status = record
                .and_then(|r| r.budgets.iter().find(|b| b.setting == *setting))
                .cloned()
                .unwrap_or_else(|| {
                    let snapshot = record.and_then(|r| {
                        snapshots(r)
                            .into_iter()
                            .find(|(id, _)| *id == setting.environment_id)
                            .map(|(_, s)| s)
                    });
                    let group = snapshot.and_then(|s| {
                        s.groups.iter().find(|g| {
                            g.scope == "worktree" && g.worktree_id == Some(setting.worktree_id)
                        })
                    });
                    let complete = snapshot.is_some_and(|s| s.coverage.inventory_complete)
                        && group.is_some_and(|g| g.totals.missing_accounting == 0);
                    let mut status = budget_status(setting, group, complete);
                    if setting.limit_bytes.is_some() {
                        status.state = "awaiting_sample".into();
                    }
                    status
                });
            if setting.limit_bytes.is_none() {
                status.state = "disabled".into();
                status.warning_active = false;
            } else if !enabled {
                status.state = "diagnostics_disabled".into();
                status.warning_active = false;
            } else if record.is_none_or(|r| {
                unix_ms()
                    .checked_sub(r.captured_unix_ms)
                    .is_none_or(|age| age > INTERVAL_SECONDS * 3000)
            }) {
                status.state = "awaiting_sample".into();
                status.warning_active = false;
            }
            status
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_workspace_backend::memory::{Coverage, GroupSummary, ProcessMemory, Totals};
    const MIB: u64 = 1024 * 1024;
    fn setting() -> BudgetSetting {
        BudgetSetting {
            environment_id: Uuid::from_u128(1),
            worktree_id: Uuid::from_u128(2),
            limit_bytes: Some(100 * MIB),
        }
    }
    fn record(time: u64, start: u64, used: u64) -> Record {
        let b = setting();
        Record {
            record_type: "sample",
            captured_unix_ms: time,
            host_environment_id: b.environment_id,
            host: Some(Snapshot {
                source_id: "boot:run".into(),
                platform: "linux".into(),
                coverage: Coverage {
                    inventory_complete: true,
                    ..Default::default()
                },
                groups: vec![GroupSummary {
                    scope: "worktree".into(),
                    worktree_id: Some(b.worktree_id),
                    totals: Totals {
                        processes: 1,
                        resident_bytes: used,
                        accounted_bytes: used,
                        missing_pss: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                processes: vec![ProcessMemory {
                    pid: 10,
                    start_id: Some(start),
                    lifetime_id: Some(format!("boot:run:10:{start}")),
                    worktree_id: Some(b.worktree_id),
                    resident_bytes: Some(used),
                    cpu_time_ms: Some(time / 10),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    #[test]
    fn budgets_warn_once_recover_with_hysteresis_and_rearm() {
        let mut t = Tracker::default();
        let b = setting();
        let mut a = record(1000, 1, 110 * MIB);
        t.update(&mut a, None, std::slice::from_ref(&b));
        assert_eq!(a.budgets[0].state, "exceeded");
        assert!(a.budgets[0].fallback_rss);
        assert_eq!(
            a.events
                .iter()
                .filter(|e| e.kind == "budget_exceeded")
                .count(),
            1
        );
        let mut c = record(31000, 1, 105 * MIB);
        t.update(&mut c, Some(&a), std::slice::from_ref(&b));
        assert!(!c.events.iter().any(|e| e.kind == "budget_exceeded"));
        let mut d = record(61000, 1, 95 * MIB);
        t.update(&mut d, Some(&c), std::slice::from_ref(&b));
        assert_eq!(d.budgets[0].state, "hysteresis");
        let mut e = record(91000, 1, 90 * MIB);
        t.update(&mut e, Some(&d), std::slice::from_ref(&b));
        assert!(e.events.iter().any(|e| e.kind == "budget_recovered"));
        let mut f = record(121000, 1, 120 * MIB);
        t.update(&mut f, Some(&e), std::slice::from_ref(&b));
        assert!(f.events.iter().any(|e| e.kind == "budget_exceeded"));
        assert_eq!(f.budgets[0].peak_accounted_bytes, 120 * MIB);
        assert_eq!(
            statuses(&[b], Some(&f), false)[0].state,
            "diagnostics_disabled"
        );
    }
    #[test]
    fn missing_metrics_never_confirm_below_budget_and_environment_scope_is_exact() {
        let mut t = Tracker::default();
        let b = setting();
        let mut a = record(1000, 1, 50 * MIB);
        a.host.as_mut().unwrap().groups[0].totals.missing_accounting = 1;
        t.update(&mut a, None, std::slice::from_ref(&b));
        assert_eq!(a.budgets[0].state, "incomplete");
        assert!(!a.budgets[0].complete);
        let other = BudgetSetting {
            environment_id: Uuid::from_u128(3),
            ..b.clone()
        };
        let mut c = record(31000, 1, 50 * MIB);
        t.update(&mut c, Some(&a), &[other]);
        assert_eq!(c.budgets[0].state, "unavailable_or_unattributed");
        assert!(c.budgets[0].accounted_bytes.is_none());
        let mut d = record(61000, 1, 50 * MIB);
        d.host.as_mut().unwrap().coverage.inventory_complete = false;
        t.update(&mut d, Some(&c), &[b]);
        assert_eq!(d.budgets[0].state, "incomplete");
    }
    #[test]
    fn pid_reuse_source_restart_gaps_and_observation_loss_reset_deltas() {
        let mut t = Tracker::default();
        let mut a = record(1000, 1, 10);
        t.update(&mut a, None, &[]);
        let mut b = record(31000, 1, 20);
        t.update(&mut b, Some(&a), &[]);
        let p = &b.host.as_ref().unwrap().processes[0];
        assert_eq!(p.resident_delta_bytes, Some(10));
        assert_eq!(p.cpu_delta_ms, Some(3000));
        assert_eq!(p.cpu_percent, Some(10.0));
        let mut c = record(61000, 2, 30);
        t.update(&mut c, Some(&b), &[]);
        assert!(c.host.as_ref().unwrap().processes[0].cpu_delta_ms.is_none());
        assert!(c
            .events
            .iter()
            .any(|e| e.kind == "process_unobserved" && e.reason.contains("not_proof_of_exit")));
        let mut d = record(161001, 2, 40);
        t.update(&mut d, Some(&c), &[]);
        assert!(d.events.iter().any(|e| e.kind == "observation_gap"));
        assert!(d.host.as_ref().unwrap().processes[0]
            .resident_delta_bytes
            .is_none());
        let mut e = record(191001, 2, 40);
        e.host.as_mut().unwrap().source_id = "new-boot:new-run".into();
        t.update(&mut e, Some(&d), &[]);
        assert!(e.events.iter().any(|e| e.kind == "source_changed"));
        assert!(e.host.as_ref().unwrap().processes[0].cpu_delta_ms.is_none());
        let mut f = record(221001, 2, 40);
        f.host = None;
        t.update(&mut f, Some(&e), &[setting()]);
        assert!(f.events.iter().any(|e| e.kind == "environment_unavailable"));
        assert_eq!(f.budgets[0].state, "unavailable_or_unattributed");
    }
    #[test]
    fn ownership_changes_and_disabled_budget_are_explicit() {
        let mut t = Tracker::default();
        let mut a = record(1000, 1, 150 * MIB);
        t.update(&mut a, None, &[setting()]);
        let mut b = record(31000, 1, 160 * MIB);
        b.host.as_mut().unwrap().processes[0].worktree_id = None;
        let off = BudgetSetting {
            limit_bytes: None,
            ..setting()
        };
        t.reset_budget(&off);
        t.update(&mut b, Some(&a), &[off]);
        assert!(b
            .events
            .iter()
            .any(|e| e.kind == "owner_changed"
                && e.previous_worktree_id == Some(setting().worktree_id)));
        assert_eq!(b.budgets[0].state, "disabled");
        assert!(!b.budgets[0].warning_active);
    }
    #[test]
    fn event_limit_keeps_budget_warning_and_edit_rearms_it() {
        let mut t = Tracker::default();
        let b = setting();
        let mut a = record(1000, 1, 110 * MIB);
        a.host.as_mut().unwrap().processes = (0..1100)
            .map(|pid| ProcessMemory {
                pid,
                lifetime_id: Some(format!("run:{pid}:1")),
                ..Default::default()
            })
            .collect();
        t.update(&mut a, None, std::slice::from_ref(&b));
        assert_eq!(a.events.len(), 1024);
        assert!(a.omitted_events > 0);
        assert!(a.events.iter().any(|e| e.kind == "budget_exceeded"));
        t.reset_budget(&b);
        let mut c = record(31000, 1, 110 * MIB);
        t.update(&mut c, Some(&a), std::slice::from_ref(&b));
        assert!(c.events.iter().any(|e| e.kind == "budget_exceeded"));
    }
}
