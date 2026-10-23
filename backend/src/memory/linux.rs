use super::*;
use std::{fs, path::Path};

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
    };
    let mut processes = vec![];
    // sysconf is read-only; these values describe the kernel's accounting units.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
                continue;
            };
            if let Ok(status) = fs::read_to_string(entry.path().join("status")) {
                if let Some(mut p) = status_process(pid, &status) {
                    if let Ok(stat) = fs::read_to_string(entry.path().join("stat")) {
                        stat_fields(&mut p, &stat, ticks);
                    }
                    processes.push(p);
                }
            }
        }
    } else {
        errors.push("proc_inventory_unavailable".into());
    }
    let scanned_processes = processes.len();
    let (mut processes, omitted_processes) =
        select(processes, &terminals, std::process::id(), true);
    let mut pss_readable_processes = 0;
    for p in &mut processes {
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
                pss_readable_processes += usize::from(p.pss_bytes.is_some());
            }
            Err(_) => p.metrics_error = Some("pss_unavailable".into()),
        }
        // Do not combine metrics from different lifetimes when a PID is reused.
        if let Ok(stat) = fs::read_to_string(dir.join("stat")) {
            let mut current = ProcessMemory::default();
            stat_fields(&mut current, &stat, ticks);
            if current.start_id != p.start_id {
                p.pss_bytes = None;
                p.private_bytes = None;
                p.metrics_error = Some("process_changed".into());
            }
        }
    }
    Snapshot {
        platform: "linux".into(),
        collector_pid: std::process::id(),
        system,
        processes,
        terminals,
        scanned_processes,
        omitted_processes,
        pss_readable_processes,
        errors,
    }
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
