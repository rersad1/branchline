use super::*;
use std::mem::{size_of, zeroed};
use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;
use windows_sys::Win32::{
    Foundation::*,
    System::{Diagnostics::ToolHelp::*, ProcessStatus::*, SystemInformation::*, Threading::*},
};

pub(super) fn sample(terminals: Vec<TerminalMemory>) -> Snapshot {
    let mut system = SystemMemory::default();
    let mut errors = vec![];
    let mut processes = vec![];
    // All handles are read-only and closed in this scope. No debug privilege is requested.
    unsafe {
        let mut memory: MEMORYSTATUSEX = zeroed();
        memory.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut memory) != 0 {
            system.total_bytes = Some(memory.ullTotalPhys);
            system.available_bytes = Some(memory.ullAvailPhys);
        } else {
            errors.push("host_memory_unavailable".into());
        }
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != INVALID_HANDLE_VALUE {
            let mut entry: PROCESSENTRY32W = zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                processes.push(ProcessMemory {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    name: bounded_name(&String::from_utf16_lossy(&entry.szExeFile[..len])),
                    threads: entry.cntThreads,
                    ..Default::default()
                });
                ok = Process32NextW(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
        } else {
            errors.push("host_inventory_unavailable".into());
        }
    }
    let scanned_processes = processes.len();
    // Mark relationships before opening processes; unrelated Windows applications are excluded.
    let (mut processes, omitted_processes) =
        select(processes, &terminals, std::process::id(), false);
    for p in &mut processes {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, p.pid);
            if process.is_null() {
                p.metrics_error = Some("process_access_denied".into());
                continue;
            }
            let mut memory: PROCESS_MEMORY_COUNTERS_EX = zeroed();
            memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if K32GetProcessMemoryInfo(
                process,
                &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                memory.cb,
            ) != 0
            {
                p.resident_bytes = Some(memory.WorkingSetSize as u64);
                p.private_bytes = Some(memory.PrivateUsage as u64);
            } else {
                p.metrics_error = Some("process_memory_unavailable".into());
            }
            let (mut created, mut exited, mut kernel, mut user): (
                FILETIME,
                FILETIME,
                FILETIME,
                FILETIME,
            ) = (zeroed(), zeroed(), zeroed(), zeroed());
            let units = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            if GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
                p.start_id = Some(units(created));
                p.cpu_time_ms = Some((units(kernel) + units(user)) / 10_000);
            }
            CloseHandle(process);
        }
    }
    // VmmemWSL cannot be opened by an ordinary user. Its working set is available
    // in the read-only system process table without a process handle or elevation.
    if processes.iter().any(|p| p.resident_bytes.is_none()) {
        match system_working_sets() {
            Ok(working_sets) => {
                for p in &mut processes {
                    if p.resident_bytes.is_none() {
                        if let Some(bytes) = working_sets.get(&p.pid) {
                            p.resident_bytes = Some(*bytes);
                            p.metrics_error =
                                Some("private_and_lifetime_metrics_unavailable".into());
                        }
                    }
                }
            }
            Err(()) => errors.push("system_process_memory_unavailable".into()),
        }
    }
    processes.sort_by_key(|p| std::cmp::Reverse(p.resident_bytes.unwrap_or(0)));
    Snapshot {
        platform: "windows".into(),
        collector_pid: std::process::id(),
        system,
        processes,
        terminals,
        scanned_processes,
        omitted_processes,
        pss_readable_processes: 0,
        errors,
    }
}

fn system_working_sets() -> Result<HashMap<u32, u64>, ()> {
    let mut bytes = 256 * 1024usize;
    while bytes <= 16 * 1024 * 1024 {
        // Aligned backing storage; every record is range-checked before reading.
        let mut storage = vec![0u64; bytes.div_ceil(8)];
        let mut needed = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                storage.as_mut_ptr().cast(),
                bytes as u32,
                &mut needed,
            )
        };
        if status as u32 == 0xc0000004 {
            // STATUS_INFO_LENGTH_MISMATCH
            bytes = (needed as usize).saturating_add(65536).max(bytes * 2);
            continue;
        }
        if status < 0 || needed as usize > bytes {
            return Err(());
        }
        let used = needed as usize;
        let mut offset = 0usize;
        let mut result = HashMap::new();
        loop {
            if offset
                .checked_add(size_of::<SYSTEM_PROCESS_INFORMATION>())
                .ok_or(())?
                > used
            {
                return Err(());
            }
            // Only scalar PID/working-set/offset fields are read; embedded pointers
            // (including the image name) are never dereferenced.
            let row = unsafe {
                std::ptr::read_unaligned(
                    storage
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<SYSTEM_PROCESS_INFORMATION>(),
                )
            };
            if let Ok(pid) = u32::try_from(row.UniqueProcessId as usize) {
                result.insert(pid, row.WorkingSetSize as u64);
            }
            if row.NextEntryOffset == 0 {
                return Ok(result);
            }
            if (row.NextEntryOffset as usize) < size_of::<SYSTEM_PROCESS_INFORMATION>() {
                return Err(());
            }
            offset = offset.checked_add(row.NextEntryOffset as usize).ok_or(())?;
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_table_provides_memory_without_opening_process_handles() {
        assert!(
            system_working_sets()
                .unwrap()
                .get(&std::process::id())
                .copied()
                .unwrap_or(0)
                > 0
        );
    }
}
