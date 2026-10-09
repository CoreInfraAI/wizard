use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use alloc::sync::Arc;
use anyhow::{Context as _, Result};
use core::time::Duration;
use serde::Serialize;
use tauri::Manager as _;
use tauri_plugin_updater::{Update, UpdaterExt as _};

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_hours(1);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum UpdateState {
    Checking,
    Installing,
    UpToDate,
    Available { version: String },
    Failed { message: String },
}

impl UpdateState {
    fn is_in_progress(&self) -> bool {
        matches!(self, Self::Checking | Self::Installing)
    }
}

pub(crate) fn start(app: tauri::AppHandle) {
    start_update(app.clone());

    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
            check_in_background(&app, &state).await;
        }
    });
}

#[tauri::command]
pub(crate) async fn get_update_state(
    last_revision: Option<u32>,
    state: tauri::State<'_, Arc<RevisionSignal<UpdateState>>>,
) -> Result<RevisionSnapshot<UpdateState>, String> {
    // state.update(|current| {
    //     (!current.is_in_progress()).then(|| UpdateState::Failed { message: "test update error".to_owned() })
    //     // (!current.is_in_progress()).then(|| UpdateState::Available { version: "0.0.0".to_owned() })
    // });
    state
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub(crate) fn request_update(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    // Claim the next update atomically to prevent concurrent installations.
    let Some(_) = state.update(|current| {
        if current.is_in_progress() {
            return None;
        }
        Some(UpdateState::Checking)
    }) else {
        return;
    };
    start_update(app);
}

#[tauri::command]
pub(crate) fn retry_update_check(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    if !matches!(state.current().state, UpdateState::Failed { .. }) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        check_in_background(&app, &state).await;
    });
}

fn start_update(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    tauri::async_runtime::spawn(async move {
        if let Err(error) = check_and_install_update(&app, &state).await {
            log::error!("application update failed: {error:#}");
            state.notify(UpdateState::Failed {
                message: format!("{error:#}"),
            });
        }
    });
}

async fn check_and_install_update(
    app: &tauri::AppHandle,
    state: &RevisionSignal<UpdateState>,
) -> Result<()> {
    log::info!("checking for application updates");

    let Some(mut update) = find_update(app).await? else {
        log::info!("application is up to date");
        state.notify(UpdateState::UpToDate);
        return Ok(());
    };
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);

    log::info!("downloading application update {}", update.version);
    state.notify(UpdateState::Installing);

    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .context("Не удалось скачать обновление")?;

    tauri::async_runtime::spawn_blocking(move || update.install(bytes))
        .await
        .context("Не удалось выполнить задачу установки обновления")?
        .context("Не удалось установить обновление")?;

    log::info!("application update installed; restarting");
    app.restart();
}

async fn check_in_background(app: &tauri::AppHandle, state: &RevisionSignal<UpdateState>) {
    log::info!("checking for application updates in background");
    let next = match find_update(app).await {
        Ok(Some(update)) => {
            log::info!("application update {} is available", update.version);
            UpdateState::Available {
                version: update.version,
            }
        }
        Ok(None) => UpdateState::UpToDate,
        Err(error) => {
            log::error!("background update check failed: {error:#}");
            return;
        }
    };
    // Do not overwrite an installation request accepted during the network check.
    state.update(|current| {
        if current.is_in_progress() || current == &next {
            return None;
        }
        Some(next)
    });
}

async fn find_update(app: &tauri::AppHandle) -> Result<Option<Update>> {
    let updater = app
        .updater_builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .build()
        .context("Не удалось подготовить проверку обновлений")?;

    updater
        .check()
        .await
        .context("Не удалось проверить обновления")
}
