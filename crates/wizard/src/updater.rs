use alloc::sync::Arc;
use core::time::Duration;

use tauri::Manager as _;
use tauri_plugin_updater::{Error as UpdaterError, UpdaterExt as _};
use tokio::sync::watch;

const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(30);

pub(crate) struct StartupUpdateState {
    completion: watch::Sender<bool>,
}

impl Default for StartupUpdateState {
    fn default() -> Self {
        let (completion, _) = watch::channel(false);
        Self { completion }
    }
}

impl StartupUpdateState {
    fn complete(&self) {
        self.completion.send_replace(true);
    }

    async fn wait(&self) {
        let mut completion = self.completion.subscribe();
        loop {
            if *completion.borrow_and_update() {
                return;
            }
            if completion.changed().await.is_err() {
                return;
            }
        }
    }
}

pub(crate) fn start(app: tauri::AppHandle) {
    let state = Arc::clone(app.state::<Arc<StartupUpdateState>>().inner());
    tauri::async_runtime::spawn(async move {
        check_and_install_update(app).await;
        state.complete();
    });
}

#[tauri::command]
pub(crate) async fn wait_for_startup_update(app: tauri::AppHandle) {
    let state = Arc::clone(app.state::<Arc<StartupUpdateState>>().inner());
    state.wait().await;
}

async fn check_and_install_update(app: tauri::AppHandle) {
    log::info!("checking for application updates");

    let updater = match app.updater_builder().timeout(UPDATE_CHECK_TIMEOUT).build() {
        Ok(updater) => updater,
        Err(UpdaterError::EmptyEndpoints) => {
            log::error!("failed to initialize updater: no update endpoints are configured");
            return;
        }
        Err(error) => {
            log::error!("failed to initialize updater: {error}");
            return;
        }
    };

    let mut update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            log::info!("application is up to date");
            return;
        }
        Err(error) => {
            log::error!("failed to check for updates: {error}");
            return;
        }
    };

    log::info!("downloading application update {}", update.version);
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);
    if let Err(error) = update.download_and_install(|_, _| {}, || {}).await {
        log::error!("failed to install update: {error}");
        return;
    }

    log::info!("application update installed; restarting");
    app.restart();
}
