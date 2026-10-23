//! Fixed-content native toasts. No provider strings, prompt or credentials reach Windows.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub environment_id: Uuid,
    pub project_id: Uuid,
    pub worktree_id: Uuid,
    pub instance_id: Uuid,
    pub session_id: String,
    pub sequence: u64,
}
#[cfg(windows)]
#[derive(Default)]
pub struct Notifications(std::sync::Mutex<Inner>);
#[cfg(windows)]
#[derive(Default)]
struct Inner {
    closed: bool,
    toasts: std::collections::VecDeque<(
        windows::UI::Notifications::ToastNotifier,
        windows::UI::Notifications::ToastNotification,
        i64,
        i64,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
    seen: std::collections::VecDeque<String>,
    recent: std::collections::VecDeque<std::time::Instant>,
    // Keep the MTA alive while Windows can call the retained event handlers.
    mta: Option<MtaLifetime>,
}
#[cfg(windows)]
struct MtaLifetime(windows::Win32::System::Com::CO_MTA_USAGE_COOKIE);
// The opaque process MTA usage cookie may be released from any thread.
#[cfg(windows)]
unsafe impl Send for MtaLifetime {}
#[cfg(windows)]
impl Drop for MtaLifetime {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Com::CoDecrementMTAUsage(self.0);
        }
    }
}
#[cfg(windows)]
impl Notifications {
    pub fn clear(&self) {
        let mut inner = self.0.lock().unwrap();
        inner.closed = true;
        for (notifier, toast, token, failed, active) in inner.toasts.drain(..) {
            active.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = toast.RemoveActivated(token);
            let _ = toast.RemoveFailed(failed);
            let _ = notifier.Hide(&toast);
        }
        inner.mta.take();
    }
    pub fn show(
        &self,
        app: tauri::AppHandle,
        target: Target,
        label: &str,
    ) -> Result<Option<String>, String> {
        use tauri::{Emitter, Manager};
        use windows::{
            core::{IInspectable, HSTRING},
            Data::Xml::Dom::XmlDocument,
            Foundation::TypedEventHandler,
            UI::Notifications::*,
        };
        let failure = |stage: &str, error: windows::core::Error| {
            format!("{stage}: 0x{:08X}", error.code().0 as u32)
        };
        let key = serde_json::to_string(&target).map_err(|_| "target".to_owned())?;
        let mut inner = self.0.lock().map_err(|_| "state".to_owned())?;
        if inner.closed {
            return Err("closed".into());
        }
        let now = std::time::Instant::now();
        inner
            .recent
            .retain(|time| now.duration_since(*time).as_secs() < 30);
        if inner.seen.contains(&key) || inner.recent.len() >= 3 {
            return Ok(None);
        }
        if inner.mta.is_none() {
            inner.mta = Some(MtaLifetime(
                unsafe { windows::Win32::System::Com::CoIncrementMTAUsage() }
                    .map_err(|e| failure("CoIncrementMTAUsage", e))?,
            ));
        }
        // Initialize this calling thread as well as retaining the process MTA.
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )
        }
        .map_err(|e| failure("RoInitialize", e))?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    windows::Win32::System::WinRT::RoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        // Match the installed shortcut's identity before asking Windows for its settings.
        unsafe {
            windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(
                &app.config().identifier,
            ))
        }
        .map_err(|e| failure("AppUserModelID", e))?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            &app.config().identifier,
        ))
        .map_err(|e| failure("CreateToastNotifier", e))?;
        // Windows can return E_ELEMENTNOTFOUND before a sender's first Show.
        // A failed availability query is not an OS policy denial. Show still enforces
        // Windows policy; its acceptance alone never certifies visual delivery.
        let diagnostic = match notifier.Setting() {
            Ok(setting) if setting == NotificationSetting::Enabled => None,
            Ok(setting) => return Err(format!("NotificationSetting={}", setting.0)),
            Err(error) if error.code().0 as u32 == 0x80070490 => Some(failure("Setting", error)),
            Err(error) => return Err(failure("Setting", error)),
        };
        let xml = XmlDocument::new().map_err(|e| failure("XmlDocument", e))?;
        xml.LoadXml(&HSTRING::from(format!("<toast duration=\"short\"><visual><binding template=\"ToastGeneric\"><text>Branchline</text><text>{label}</text></binding></visual></toast>"))).map_err(|e|failure("LoadXml",e))?;
        let toast =
            ToastNotification::CreateToastNotification(&xml).map_err(|e| failure("Toast", e))?;
        let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let activation_active = active.clone();
        let activation_app = app.clone();
        let token = toast
            .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, _| {
                    if !activation_active.load(std::sync::atomic::Ordering::SeqCst) {
                        return Ok(());
                    }
                    if let Some(window) = activation_app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                    let _ = activation_app.emit("attention-open", &target);
                    Ok(())
                },
            ))
            .map_err(|e| failure("Activated", e))?;
        let failed_active = active.clone();
        let failed = match toast.Failed(
            &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, args| {
                if failed_active.load(std::sync::atomic::Ordering::SeqCst) {
                    let code = args
                        .as_ref()
                        .and_then(|args| args.ErrorCode().ok())
                        .map(|code| format!("0x{:08X}", code.0 as u32))
                        .unwrap_or_else(|| "unknown".into());
                    let _ = app.emit(
                        "attention-unavailable",
                        format!("Windows toast Failed: {code}. Attention остаётся в приложении."),
                    );
                }
                Ok(())
            }),
        ) {
            Ok(token) => token,
            Err(error) => {
                let _ = toast.RemoveActivated(token);
                return Err(failure("Failed handler", error));
            }
        };
        if let Err(error) = notifier.Show(&toast) {
            active.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = toast.RemoveActivated(token);
            let _ = toast.RemoveFailed(failed);
            return Err(failure("Show", error));
        }
        inner.recent.push_back(now);
        inner.seen.push_back(key);
        while inner.seen.len() > 256 {
            inner.seen.pop_front();
        }
        inner
            .toasts
            .push_back((notifier, toast, token, failed, active));
        while inner.toasts.len() > 24 {
            if let Some((notifier, toast, token, failed, active)) = inner.toasts.pop_front() {
                active.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = toast.RemoveActivated(token);
                let _ = toast.RemoveFailed(failed);
                let _ = notifier.Hide(&toast);
            }
        }
        Ok(diagnostic)
    }
}
#[cfg(not(windows))]
#[derive(Default)]
pub struct Notifications;
#[cfg(not(windows))]
impl Notifications {
    pub fn clear(&self) {}
    pub fn show(&self, _: tauri::AppHandle, _: Target, _: &str) -> Result<Option<String>, String> {
        Err("unsupported_platform".into())
    }
}
