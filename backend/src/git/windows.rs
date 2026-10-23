//! Owned Windows job: suspended spawn prevents descendants escaping assignment.
use super::GitError;
use std::{
    fs::OpenOptions,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};

pub struct Job(HANDLE);
// SAFETY: the job handle is owned; Windows handles can cross threads.
unsafe impl Send for Job {}
impl Job {
    pub fn assign_std_and_resume(&self, child: &std::process::Child) -> Result<(), GitError> {
        // SAFETY: only our suspended shell is passed; no PID supplied by UI.
        unsafe {
            if AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) == 0 {
                return Err(GitError::Io);
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(GitError::Io);
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = Thread32First(snapshot, &mut entry);
            let mut result = Err(GitError::Io);
            while found != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        if ResumeThread(thread) != u32::MAX {
                            result = Ok(());
                        }
                        CloseHandle(thread);
                    }
                    break;
                }
                found = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            result
        }
    }
    pub fn new() -> Result<Self, GitError> {
        // SAFETY: null optional security/name, valid information buffer.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(GitError::Io);
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(GitError::Io);
            }
            Ok(job)
        }
    }
    pub fn assign_and_resume(&self, child: &tokio::process::Child) -> Result<(), GitError> {
        // SAFETY: live child process handle and owned job. Child was spawned suspended.
        unsafe {
            if AssignProcessToJobObject(self.0, child.raw_handle().ok_or(GitError::Io)? as HANDLE)
                == 0
            {
                return Err(GitError::Io);
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(GitError::Io);
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of_val(&entry) as u32;
            let mut found = Thread32First(snapshot, &mut entry);
            let mut result = Err(GitError::Io);
            while found != 0 {
                if Some(entry.th32OwnerProcessID) == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        if ResumeThread(thread) != u32::MAX {
                            result = Ok(());
                        }
                        CloseHandle(thread);
                    }
                    break;
                }
                found = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            result
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: uniquely owned job handle. Closing kills all owned descendants.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub fn file_identity(path: &Path) -> Option<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    // SAFETY: valid file handle and initialized output buffer.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) == 0 {
            return None;
        }
        Some(format!(
            "{}:{}:{}:{}:{}",
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime
        ))
    }
}

/// Stable OS installation identity; never use the mutable computer name as an ID.
pub fn machine_identity() -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::*;
    let key: Vec<u16> = std::ffi::OsStr::new(r"SOFTWARE\Microsoft\Cryptography")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let value: Vec<u16> = std::ffi::OsStr::new("MachineGuid")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut buffer = [0u16; 128];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: bounded writable buffer, NUL-terminated key and value, read-only query.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    String::from_utf16(&buffer[..buffer.iter().position(|v| *v == 0)?])
        .ok()
        .filter(|v| !v.is_empty())
}

/// WSL registration GUID detects re-imports even when /etc/machine-id was cloned.
pub fn distro_registration(distro: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::*;
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    let root = wide(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
    let mut key = std::ptr::null_mut();
    // SAFETY: read-only registry handle, terminated strings and bounded buffers.
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, root.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return None;
        }
        let mut found = None;
        for index in 0..256 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            if RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0
            {
                break;
            }
            let subkey = String::from_utf16(&name[..length as usize]).ok();
            let mut value = [0u16; 256];
            let mut size = std::mem::size_of_val(&value) as u32;
            if RegGetValueW(
                key,
                name.as_ptr(),
                wide("DistributionName").as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                value.as_mut_ptr() as *mut _,
                &mut size,
            ) == 0
            {
                let end = value.iter().position(|c| *c == 0).unwrap_or(value.len());
                if String::from_utf16(&value[..end]).ok().as_deref() == Some(distro) {
                    found = subkey;
                    break;
                }
            }
        }
        RegCloseKey(key);
        found
    }
}
