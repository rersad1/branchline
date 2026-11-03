use super::*;
use std::{fs, io::Read, path::Path};

fn pressure(text: &str, kind: &str) -> Option<Pressure> {
    let line = text.lines().find(|l| l.starts_with(kind))?;
    let fields: HashMap<_, _> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|s| s.split_once('='))
        .collect();
    Some(Pressure {
        avg10: fields.get("avg10")?.parse().ok()?,
        avg60: fields.get("avg60")?.parse().ok()?,
        avg300: fields.get("avg300")?.parse().ok()?,
        total_us: fields.get("total")?.parse().ok()?,
    })
}

// Fixed vocabulary only. Never include tokens, paths, properties or arguments in
// the result/error. At most 16 KiB is read, only for Java, only during this sample.
fn java_role(bytes: &[u8]) -> &'static str {
    let args = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if args.contains("gradleworkermain")
        || args.contains("surefirebooter")
        || args.contains("junit")
    {
        "test_worker"
    } else if args.contains("gradledaemon") {
        "gradle_daemon"
    } else if args.contains("maven") || args.contains("gradlewrappermain") {
        "build_tool"
    } else if args.contains("jdt.ls") || args.contains("equinox.launcher") {
        "language_server"
    } else if args.contains("kotlincompiledaemon") {
        "compiler_daemon"
    } else {
        "unknown_jvm"
    }
}

fn fields(text: &str) -> HashMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key, value.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}

pub(super) fn sample(terminals: Vec<TerminalMemory>) -> Snapshot {
    let mut errors = vec![];
    let mem = fs::read_to_string("/proc/meminfo").unwrap_or_else(|_| {
        errors.push("meminfo_unavailable".into());
        String::new()
    });
    let m = fields(&mem);
    let kb = |key| m.get(key).map(|n| n.saturating_mul(1024));
    let psi = fs::read_to_string("/proc/pressure/memory").unwrap_or_default();
    if psi.is_empty() {
        errors.push("memory_pressure_unavailable".into());
    }
    let system = SystemMemory {
        total_bytes: kb("MemTotal"),
        available_bytes: kb("MemAvailable"),
        free_bytes: kb("MemFree"),
        cached_bytes: kb("Cached").zip(kb("Buffers")).map(|(a, b)| a + b),
        reclaimable_slab_bytes: kb("SReclaimable"),
        unreclaimable_slab_bytes: kb("SUnreclaim"),
        anonymous_bytes: kb("AnonPages"),
        shmem_bytes: kb("Shmem"),
        swap_total_bytes: kb("SwapTotal"),
        swap_free_bytes: kb("SwapFree"),
        pressure_some: pressure(&psi, "some "),
        pressure_full: pressure(&psi, "full "),
    };
    let mut processes = vec![];
    let mut coverage = Coverage {
        inventory_complete: true,
        ..Default::default()
    };
    // sysconf is read-only; these values describe the kernel's accounting units.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    coverage.inventory_complete = false;
                    if !errors.iter().any(|e| e == "proc_entry_unavailable") {
                        errors.push("proc_entry_unavailable".into());
                    }
                    continue;
                }
            };
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
                continue;
            };
            coverage.discovered_processes += 1;
            let mut lifetime = ProcessMemory::default();
            if let Ok(stat) = fs::read_to_string(entry.path().join("stat")) {
                stat_fields(&mut lifetime, &stat, ticks);
            }
            if let Ok(status) = fs::read_to_string(entry.path().join("status")) {
                if let Some(mut p) = status_process(pid, &status) {
                    if let Ok(stat) = fs::read_to_string(entry.path().join("stat")) {
                        stat_fields(&mut p, &stat, ticks);
                    }
                    if p.start_id.is_none() || p.start_id != lifetime.start_id {
                        continue;
                    }
                    if p.name == "java" {
                        let mut bytes = Vec::new();
                        if fs::File::open(entry.path().join("cmdline"))
                            .and_then(|f| f.take(16 * 1024).read_to_end(&mut bytes))
                            .is_ok()
                        {
                            p.role = java_role(&bytes).into();
                        }
                    }
                    p.container = fs::read_to_string(entry.path().join("cgroup"))
                        .ok()
                        .map(|s| {
                            ["docker", "kubepods", "libpod", "containerd"]
                                .iter()
                                .any(|v| s.contains(v))
                        });
                    processes.push(p);
                }
            }
        }
    } else {
        errors.push("proc_inventory_unavailable".into());
        coverage.inventory_complete = false;
    }
    coverage.unreadable_processes = coverage
        .discovered_processes
        .saturating_sub(processes.len());
    coverage.inventory_complete &= coverage.unreadable_processes == 0;
    let scanned_processes = processes.len();
    let (mut processes, _) = attribute(processes, &terminals, std::process::id(), true);
    prioritize(&mut processes);
    for p in processes.iter_mut().take(MAX_PROCESSES) {
        // smaps_rollup is much smaller than smaps. Failure is missing data, never zero.
        let dir = Path::new("/proc").join(p.pid.to_string());
        match fs::read_to_string(dir.join("smaps_rollup")) {
            Ok(text) => {
                let values = fields(&text);
                p.pss_bytes = values.get("Pss").map(|n| n * 1024);
                p.private_bytes = values
                    .get("Private_Clean")
                    .zip(values.get("Private_Dirty"))
                    .map(|(a, b)| (a + b + values.get("Private_Hugetlb").unwrap_or(&0)) * 1024);
            }
            Err(_) => p.metrics_error = Some("pss_unavailable".into()),
        }
        // Do not combine metrics from different lifetimes when a PID is reused.
        let stable = if let Ok(stat) = fs::read_to_string(dir.join("stat")) {
            let mut current = ProcessMemory::default();
            stat_fields(&mut current, &stat, ticks);
            current.start_id.is_some() && current.start_id == p.start_id
        } else {
            false
        };
        if !stable {
            p.resident_bytes = None;
            p.swap_bytes = None;
            p.cpu_time_ms = None;
            p.start_id = None;
            p.pss_bytes = None;
            p.private_bytes = None;
            p.metrics_error = Some("process_changed".into());
        }
    }
    finish(
        Snapshot {
            platform: "linux".into(),
            collector_pid: std::process::id(),
            system,
            terminals,
            scanned_processes,
            coverage,
            errors,
            ..Default::default()
        },
        processes,
    )
}

fn status_process(pid: u32, text: &str) -> Option<ProcessMemory> {
    let numbers = fields(text);
    let name = text
        .lines()
        .find_map(|line| line.strip_prefix("Name:"))
        .unwrap_or("unknown")
        .trim();
    Some(ProcessMemory {
        pid,
        parent_pid: (*numbers.get("PPid")?).try_into().ok()?,
        name: bounded_name(name),
        resident_bytes: numbers.get("VmRSS").map(|n| n * 1024),
        swap_bytes: numbers.get("VmSwap").map(|n| n * 1024),
        threads: (*numbers.get("Threads").unwrap_or(&0))
            .try_into()
            .unwrap_or(0),
        ..Default::default()
    })
}

fn stat_fields(p: &mut ProcessMemory, stat: &str, ticks: u64) {
    // comm may contain spaces and parentheses; fields after its final ')' are stable.
    let Some((_, tail)) = stat.rsplit_once(')') else {
        return;
    };
    let values: Vec<_> = tail.split_whitespace().collect();
    let value = |i: usize| values.get(i).and_then(|s| s.parse::<u64>().ok());
    p.start_id = value(19);
    p.cpu_time_ms = value(11)
        .zip(value(12))
        .map(|(a, b)| (a + b).saturating_mul(1000) / ticks);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_java_roles_discard_secrets_and_psi_is_optional() {
        for (text,role) in [("java\0-Dsecret=PRIVATE\0/private/project/org.gradle.launcher.daemon.bootstrap.GradleDaemon","gradle_daemon"),("java\0GradleWorkerMain\0SECRET","test_worker"),("java\0org.eclipse.equinox.launcher","language_server"),("java\0private.jar","unknown_jvm")] {
            assert_eq!(java_role(text.as_bytes()),role);
        }
        let psi = pressure(
            "some avg10=1.20 avg60=0.30 avg300=0.10 total=123\n",
            "some ",
        )
        .unwrap();
        assert_eq!(psi.avg10, 1.2);
        assert_eq!(psi.total_us, 123);
        assert!(pressure("", "full ").is_none());
    }
    #[test]
    fn parses_missing_metrics_and_weird_process_names() {
        let p = status_process(
            12,
            "Name:\topencode\nPPid:\t4\nVmRSS:\t123 kB\nThreads:\t8\n",
        )
        .unwrap();
        assert_eq!(p.resident_bytes, Some(123 * 1024));
        assert_eq!(p.swap_bytes, None);
        let mut p = ProcessMemory::default();
        let mut values = vec!["0"; 20];
        values[11] = "100";
        values[12] = "50";
        values[19] = "987";
        stat_fields(&mut p, &format!("12 (a (b) c) {}", values.join(" ")), 100);
        assert_eq!(p.cpu_time_ms, Some(1500));
        assert_eq!(p.start_id, Some(987));
    }
}
