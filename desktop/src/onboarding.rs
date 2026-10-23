use crate::runtime::Problem;

/// Runs on its own blocking STA thread; COM objects never cross threads.
#[cfg(windows)]
pub fn folder(owner: isize) -> Result<Option<String>, Problem> {
    use windows::Win32::{
        Foundation::{ERROR_CANCELLED, HWND},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{
            FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PICKFOLDERS,
            SIGDN_FILESYSPATH,
        },
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|_| Problem::new("dialog_failed"))?;
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _com = Com;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|_| Problem::new("dialog_failed"))?;
        let options = dialog
            .GetOptions()
            .map_err(|_| Problem::new("dialog_failed"))?;
        dialog
            .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR)
            .map_err(|_| Problem::new("dialog_failed"))?;
        if let Err(error) = dialog.Show(Some(HWND(owner as *mut _))) {
            if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(Problem::new("dialog_failed"));
        }
        let item = dialog
            .GetResult()
            .map_err(|_| Problem::new("dialog_failed"))?;
        let value = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|_| Problem::new("dialog_failed"))?;
        let result = value
            .to_string()
            .map_err(|_| Problem::new("invalid_request"));
        CoTaskMemFree(Some(value.0 as *const _));
        result.map(Some)
    }
}

pub async fn distros() -> Result<Vec<String>, Problem> {
    let mut command = tokio::process::Command::new("wsl.exe");
    command.args(["--list", "--quiet"]).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| Problem::new("startup_failed"))?
        .map_err(|_| Problem::new("startup_failed"))?;
    if !output.status.success() || output.stdout.len() > 65536 {
        return Err(Problem::new("startup_failed"));
    }
    let bytes = output.stdout;
    let text = if bytes.contains(&0) {
        String::from_utf16(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| Problem::new("invalid_response"))?
    } else {
        String::from_utf8(bytes).map_err(|_| Problem::new("invalid_response"))?
    };
    Ok(text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && !s.starts_with('-')
                && !s.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .collect())
}
