//! Owned PTY supervisor: containment is established before launching any user shell.
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}
#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> i32>, add: i32) -> i32;
}
#[cfg(windows)]
extern "system" fn console_control(event: u32) -> i32 {
    // A per-process handler, not the inheritable NULL/ignore flag: PowerShell still
    // receives Ctrl+C while the supervisor stays alive to own the shell's job.
    i32::from(event == 0 || event == 1)
}

/// Only fixed, environment-local shells; arbitrary commands enter through explicit PTY input.
pub fn run() -> i32 {
    #[cfg(windows)]
    // SAFETY: process-local static handler; no captured pointers or allocation.
    if unsafe { SetConsoleCtrlHandler(Some(console_control), 1) } == 0 {
        return 125;
    }
    #[cfg(windows)]
    let parent = {
        use windows_sys::Win32::System::Threading::*;
        let pid = std::env::args()
            .nth(2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        if pid == 0 {
            return 125;
        }
        // Read-only lifetime handle: parent death ends only this supervisor's owned job.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return 125;
        }
        handle
    };
    #[cfg(windows)]
    let job = match crate::git::windows::Job::new() {
        Ok(j) => j,
        Err(_) => return 125,
    };
    #[cfg(unix)]
    let parent = {
        use std::os::fd::FromRawFd;
        let expected = std::env::args()
            .nth(2)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);
        // PDEATHSIG follows the spawning THREAD, which a Tokio blocking pool may retire.
        // A pidfd instead watches the whole worker lifetime, like the Windows parent handle.
        unsafe {
            libc::signal(libc::SIGTERM, stop as *const () as usize);
            libc::signal(libc::SIGHUP, stop as *const () as usize);
            if expected <= 0
                || libc::getppid() != expected
                || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
            {
                return 125;
            }
            let fd = libc::syscall(libc::SYS_pidfd_open, expected, 0) as i32;
            if fd < 0 {
                return 125;
            }
            let handle = std::os::fd::OwnedFd::from_raw_fd(fd);
            if libc::getppid() != expected {
                return 125;
            }
            handle
        }
    };
    #[cfg(windows)]
    let mut command = {
        // SystemRoot is host configuration, never IPC input. Use the OS-provided executable.
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let mut c = Command::new(
            std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        );
        c.arg("-NoLogo");
        if std::env::var("BRANCHLINE_SHELL_INTEGRATION").as_deref() != Ok("0") {
            // Fixed embedded source, not IPC/path interpolation. -NoProfile permits
            // comparing the default prompt before loading the normal profiles once.
            let source = include_str!("shell/powershell.ps1");
            let bytes: Vec<u8> = source.encode_utf16().flat_map(u16::to_le_bytes).collect();
            c.args([
                "-NoProfile",
                "-NoExit",
                "-EncodedCommand",
                &encode_base64(&bytes),
            ]);
        }
        c
    };
    #[cfg(unix)]
    let mut command = {
        let mut c = Command::new("/bin/bash");
        if std::env::var("BRANCHLINE_SHELL_INTEGRATION").as_deref() == Ok("0") {
            c.arg("-i");
        } else {
            // An inherited pipe is the rcfile: no temporary/profile file or cleanup.
            c.args([
                "-c",
                "exec /bin/bash --rcfile <(printf '%s' \"$1\") -i",
                "branchline",
                include_str!("shell/bash.bash"),
            ]);
        }
        c
    };
    if std::env::args().nth(1).as_deref() == Some("--harness-stdio") {
        let mut args = std::env::args_os().skip(3);
        let Some(executable) = args.next() else {
            return 125;
        };
        if !std::path::Path::new(&executable).is_absolute() {
            return 125;
        }
        command = Command::new(executable);
        command.args(args);
    }
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(_) => return 125,
    };
    #[cfg(windows)]
    if job.assign_std_and_resume(&child).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return 125;
    }
    let code = loop {
        #[cfg(windows)]
        if unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(parent, 0) }
            != windows_sys::Win32::Foundation::WAIT_TIMEOUT
        {
            break 130;
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut poll = libc::pollfd {
                fd: parent.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one owned descriptor, nonblocking readiness probe.
            if unsafe { libc::poll(&mut poll, 1, 0) } > 0 {
                break 130;
            }
        }
        #[cfg(unix)]
        if STOP.load(std::sync::atomic::Ordering::SeqCst) {
            break 130;
        }
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(1),
            Err(_) => break 125,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(parent);
    }
    #[cfg(unix)]
    {
        kill_descendants();
        let _ = child.wait();
        // Reap adopted children; only children of this dedicated subreaper.
        for _ in 0..50 {
            let mut status = 0;
            // SAFETY: -1 waits only on children, bounded by WNOHANG.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid < 0 {
                break;
            }
            if pid == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    code
}

#[cfg(windows)]
fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let bits = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        for shift in [18, 12] {
            result.push(TABLE[((bits >> shift) & 63) as usize] as char);
        }
        result.push(if chunk.len() > 1 {
            TABLE[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            TABLE[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

#[cfg(unix)]
fn parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
#[cfg(unix)]
fn kill_descendants() {
    // Keep the supervisor alive as the ancestry root. pidfds prevent PID reuse signaling.
    // Stop ancestors before walking children so they cannot fork during cleanup.
    fn visit(root: i32) {
        let mut queue = std::collections::VecDeque::from([root]);
        let mut owned = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while let Some(pid) = queue.pop_front() {
            let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
                continue;
            };
            for task in tasks.flatten() {
                let children =
                    std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
                for child in children
                    .split_whitespace()
                    .filter_map(|p| p.parse::<i32>().ok())
                {
                    if !seen.insert(child) {
                        continue;
                    }
                    // SAFETY: pidfd binds one lifetime, parent checked after handle acquisition.
                    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child, 0) as i32 };
                    if fd < 0 {
                        continue;
                    }
                    if parent(child) == Some(pid) {
                        unsafe {
                            libc::syscall(
                                libc::SYS_pidfd_send_signal,
                                fd,
                                libc::SIGSTOP,
                                std::ptr::null::<libc::siginfo_t>(),
                                0,
                            );
                        }
                        owned.push(fd);
                        queue.push_back(child);
                    } else {
                        unsafe {
                            libc::close(fd);
                        }
                    }
                }
            }
        }
        for fd in owned.into_iter().rev() {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd,
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
                libc::close(fd);
            }
        }
    }
    for _ in 0..3 {
        visit(std::process::id() as i32);
        std::thread::sleep(Duration::from_millis(5));
    }
}
