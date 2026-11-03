//! Fixed GitHub release source; only signed packages for this application can install.
use crate::{
    notifications,
    runtime::{Problem, Runtime},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Check,
    Download,
    Install,
}

#[derive(Clone, Serialize)]
pub struct Status {
    current_version: String,
    phase: &'static str,
    version: Option<String>,
    downloaded: u64,
    total: Option<u64>,
    error: Option<&'static str>,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION").into(),
            phase: "idle",
            version: None,
            downloaded: 0,
            total: None,
            error: None,
        }
    }
}
#[derive(Default)]
struct Inner {
    status: Status,
    update: Option<Update>,
    package: Option<Vec<u8>>,
}
#[derive(Default)]
pub struct Updates {
    inner: Mutex<Inner>,
    operation: tokio::sync::Mutex<()>,
}

fn allowed_asset(url: &str, version: &str) -> bool {
    url == format!("https://github.com/rersad1/branchline/releases/download/v{version}/Branchline_{version}_x64-setup.exe")
}
impl Updates {
    fn status(&self) -> Status {
        self.inner.lock().unwrap().status.clone()
    }
    fn emit(&self, app: &tauri::AppHandle) -> Status {
        let status = self.status();
        let _ = app.emit("update-status", &status);
        status
    }
    fn fail(&self, app: &tauri::AppHandle, message: &'static str) -> Status {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.status.phase = "error";
            inner.status.error = Some(message);
        }
        self.emit(app)
    }
    pub async fn request(&self, app: tauri::AppHandle, action: Action) -> Result<Status, Problem> {
        if matches!(action, Action::Status) {
            return Ok(self.status());
        }
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| Problem::new("operation_busy"))?;
        match action {
            Action::Status => unreachable!(),
            Action::Check => {
                // Retain the verified download until the user chooses to install it.
                if self.inner.lock().unwrap().package.is_some() {
                    return Ok(self.status());
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.status.phase = "checking";
                    inner.status.error = None;
                }
                self.emit(&app);
                let cleanup = app.clone();
                let updater = app
                    .updater_builder()
                    .timeout(Duration::from_secs(20))
                    .on_before_exit(move || {
                        cleanup.state::<notifications::Notifications>().clear();
                        let runtime = cleanup.state::<Arc<Runtime>>().inner().clone();
                        // install runs on a blocking thread, so normal async process cleanup can finish.
                        tauri::async_runtime::block_on(runtime.close());
                    })
                    .build()
                    .map_err(|_| Problem::new("operation_failed"))?;
                match updater.check().await {
                    Ok(update) => {
                        if update
                            .as_ref()
                            .is_some_and(|u| !allowed_asset(u.download_url.as_str(), &u.version))
                        {
                            return Ok(self
                                .fail(&app, "Релиз содержит неподдерживаемый пакет обновления."));
                        }
                        let mut inner = self.inner.lock().unwrap();
                        inner.status.version = update.as_ref().map(|u| u.version.clone());
                        inner.status.phase = if update.is_some() {
                            "available"
                        } else {
                            "current"
                        };
                        inner.update = update;
                        drop(inner);
                        Ok(self.emit(&app))
                    }
                    Err(_) => Ok(self.fail(
                        &app,
                        "Не удалось проверить обновления. Проверьте подключение и повторите позже.",
                    )),
                }
            }
            Action::Download => {
                let mut update = self
                    .inner
                    .lock()
                    .unwrap()
                    .update
                    .clone()
                    .ok_or_else(|| Problem::new("invalid_request"))?;
                if self.inner.lock().unwrap().package.is_some() {
                    return Ok(self.status());
                }
                update.timeout = Some(Duration::from_secs(180));
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.status.phase = "downloading";
                    inner.status.error = None;
                    inner.status.downloaded = 0;
                    inner.status.total = None;
                }
                self.emit(&app);
                let mut last_progress = Instant::now();
                let package = update
                    .download(
                        |chunk, total| {
                            {
                                let mut inner = self.inner.lock().unwrap();
                                inner.status.downloaded =
                                    inner.status.downloaded.saturating_add(chunk as u64);
                                inner.status.total = total;
                            }
                            if last_progress.elapsed() >= Duration::from_millis(200) {
                                self.emit(&app);
                                last_progress = Instant::now();
                            }
                        },
                        || {},
                    )
                    .await;
                match package {
                    Ok(bytes) => {
                        { let mut inner = self.inner.lock().unwrap(); inner.package = Some(bytes); inner.status.phase = "ready"; }
                        Ok(self.emit(&app))
                    }
                    Err(_) => Ok(self.fail(&app, "Не удалось скачать обновление или проверить его подпись. Текущая версия сохранена.")),
                }
            }
            Action::Install => {
                let (update, bytes) = {
                    let mut inner = self.inner.lock().unwrap();
                    let update = inner
                        .update
                        .clone()
                        .ok_or_else(|| Problem::new("invalid_request"))?;
                    let bytes = inner
                        .package
                        .take()
                        .ok_or_else(|| Problem::new("invalid_request"))?;
                    inner.status.phase = "installing";
                    inner.status.error = None;
                    (update, bytes)
                };
                self.emit(&app);
                // The plugin verifies the signature again before extracting the installer.
                // Its before-exit hook closes owned agents/terminals before the Windows restart.
                let result = tokio::task::spawn_blocking(move || update.install(bytes)).await;
                match result {
                    Ok(Ok(())) => Ok(self.status()),
                    _ => Ok(self.fail(&app, "Не удалось установить обновление. Перезапустите Branchline и повторите попытку.")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_the_announced_release_asset_in_the_owned_repository() {
        let expected = "https://github.com/rersad1/branchline/releases/download/v0.2.1/Branchline_0.2.1_x64-setup.exe";
        assert!(allowed_asset(expected, "0.2.1"));
        for url in [
            expected.replace("https:", "http:"),
            expected.replace("rersad1/", "someone/"),
            expected.replace("github.com", "github.com.example.invalid"),
            expected.replace("v0.2.1/", "v0.2.0/"),
            format!("{expected}?redirect=other"),
            format!("{expected}#fragment"),
            "file:///tmp/setup.exe".into(),
        ] {
            assert!(!allowed_asset(&url, "0.2.1"));
        }
        assert!(!allowed_asset(expected, "0.2.2"));
    }
}
