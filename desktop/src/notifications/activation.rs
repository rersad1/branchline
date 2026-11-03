//! Live-process COM activation for unpackaged Windows toasts. No cold-start server,
//! executable registration, user text or serialized session target is exposed to Windows.
use super::Target;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Emitter, Manager};
use uuid::Uuid;
use windows::{
    core::{implement, IUnknown, Interface, Ref, BOOL, GUID, HSTRING, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, CLASS_E_NOAGGREGATION, ERROR_ALREADY_EXISTS, E_POINTER,
            HANDLE,
        },
        System::{
            Com::{
                CoRegisterClassObject, CoRevokeClassObject, IClassFactory, IClassFactory_Impl,
                CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE,
            },
            Threading::CreateMutexW,
        },
        UI::Notifications::{
            INotificationActivationCallback, INotificationActivationCallback_Impl,
            NOTIFICATION_USER_INPUT_DATA,
        },
    },
};
use winreg::{
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE},
    RegKey,
};

pub struct Pending {
    pub target: Target,
    pub active: Arc<AtomicBool>,
    opened: AtomicBool,
}
pub struct State {
    app: tauri::AppHandle,
    targets: Mutex<HashMap<Uuid, Arc<Pending>>>,
}
impl State {
    pub fn insert(&self, token: Uuid, target: Target, active: Arc<AtomicBool>) {
        self.targets.lock().unwrap().insert(
            token,
            Arc::new(Pending {
                target,
                active,
                opened: AtomicBool::new(false),
            }),
        );
    }
    pub fn remove(&self, token: Uuid) {
        self.targets.lock().unwrap().remove(&token);
    }
    pub fn open(&self, token: Uuid) {
        let pending = self.targets.lock().unwrap().get(&token).cloned();
        let Some(pending) = pending else {
            return;
        };
        if !pending.active.load(Ordering::SeqCst) || pending.opened.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            if !pending.active.load(Ordering::SeqCst) {
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            let _ = app.emit("attention-open", &pending.target);
        });
    }
}

#[implement(INotificationActivationCallback)]
struct Callback(Arc<State>);
impl INotificationActivationCallback_Impl for Callback_Impl {
    fn Activate(
        &self,
        app_id: &PCWSTR,
        args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        count: u32,
    ) -> windows::core::Result<()> {
        // COM supplies NUL-terminated strings. Bound reads before parsing untrusted arguments.
        if count == 0
            && unsafe { bounded_string(*app_id, 128) }.as_deref()
                == Some(&self.0.app.config().identifier)
        {
            if let Some(token) =
                unsafe { bounded_string(*args, 36) }.and_then(|s| Uuid::parse_str(&s).ok())
            {
                self.0.open(token);
            }
        }
        Ok(())
    }
}
unsafe fn bounded_string(value: PCWSTR, limit: usize) -> Option<String> {
    if value.is_null() {
        return None;
    }
    for length in 0..=limit {
        if unsafe { *value.0.add(length) } == 0 {
            return String::from_utf16(unsafe { std::slice::from_raw_parts(value.0, length) }).ok();
        }
    }
    None
}

#[implement(IClassFactory)]
struct Factory(Arc<State>);
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut std::ffi::c_void,
    ) -> windows::core::Result<()> {
        if iid.is_null() || object.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *object = std::ptr::null_mut();
        }
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let callback: INotificationActivationCallback = Callback(self.0.clone()).into();
        unsafe { callback.query(iid, object).ok() }
    }
    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

struct SenderLock(HANDLE);
// This is an existence lock, not a thread-owned mutex. CloseHandle is thread-independent.
unsafe impl Send for SenderLock {}
impl Drop for SenderLock {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct Activation {
    pub state: Arc<State>,
    cookie: u32,
    key: RegKey,
    clsid: String,
    previous: Option<winreg::RegValue>,
    _sender_lock: SenderLock,
}
impl Activation {
    pub fn register(app: tauri::AppHandle) -> Result<Self, String> {
        let id = &app.config().identifier;
        // Windows has one activator per AUMID. A second running app must use its in-app
        // fallback instead of stealing another process's notification callbacks.
        let handle = unsafe {
            CreateMutexW(
                None,
                false,
                &HSTRING::from(format!("Local\\{id}.notifications")),
            )
        }
        .map_err(|e| format!("notification sender lock: {e}"))?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let sender_lock = SenderLock(handle);
        if already_exists {
            return Err("notification sender already active in another Branchline process".into());
        }
        let state = Arc::new(State {
            app: app.clone(),
            targets: Mutex::new(HashMap::new()),
        });
        let uuid = Uuid::new_v4();
        let guid = GUID::from_u128(uuid.as_u128());
        let clsid = format!("{{{uuid}}}");
        let factory: IClassFactory = Factory(state.clone()).into();
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey_with_flags(
                format!("Software\\Classes\\AppUserModelId\\{id}"),
                KEY_READ | KEY_WRITE,
            )
            .map_err(|e| format!("notification identity: {e}"))?;
        let previous = match key.get_raw_value("CustomActivator") {
            Ok(value) => Some(value),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("notification activator identity: {e}")),
        };
        let cookie = unsafe {
            CoRegisterClassObject(&guid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
        }
        .map_err(|e| format!("CoRegisterClassObject: {e}"))?;
        if let Err(e) = key.set_value("CustomActivator", &clsid) {
            unsafe {
                let _ = CoRevokeClassObject(cookie);
            }
            return Err(format!("notification activator identity: {e}"));
        }
        Ok(Self {
            state,
            cookie,
            key,
            clsid,
            previous,
            _sender_lock: sender_lock,
        })
    }
}
impl Drop for Activation {
    fn drop(&mut self) {
        for pending in self.state.targets.lock().unwrap().drain().map(|(_, p)| p) {
            pending.active.store(false, Ordering::SeqCst);
        }
        unsafe {
            let _ = CoRevokeClassObject(self.cookie);
        }
        if self
            .key
            .get_value::<String, _>("CustomActivator")
            .ok()
            .as_deref()
            == Some(&self.clsid)
        {
            if let Some(previous) = &self.previous {
                let _ = self.key.set_raw_value("CustomActivator", previous);
            } else {
                let _ = self.key.delete_value("CustomActivator");
            }
        }
    }
}
