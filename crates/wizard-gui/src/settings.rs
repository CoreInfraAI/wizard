use std::sync::Mutex;

use anyhow::{Context as _, Result, anyhow, bail};
use tauri::Manager as _;
use wizard_core::settings::{self, Settings};
use wizard_core::validate_token;

use crate::revision_signal::{RevisionSignal, RevisionSnapshot};

static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) async fn initialize_state(app: &tauri::AppHandle) -> Result<()> {
    let settings = tauri::async_runtime::spawn_blocking(settings::load_from_file)
        .await
        .context("settings initialization task failed")??;

    validate_token(&settings.coreinfra_token)?;

    if !app.manage(RevisionSignal::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_settings_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<Settings>>,
) -> Result<RevisionSnapshot<Settings>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

/// TODO: maybe update state when file changes
pub(crate) fn get_state(app: &tauri::AppHandle) -> Result<Settings> {
    let signal = app
        .try_state::<RevisionSignal<Settings>>()
        .context("settings not initialized")?;
    Ok(signal.current().state)
}

pub(crate) async fn update_state(
    app: &tauri::AppHandle,
    edit: impl FnOnce(&mut Settings) + Send + 'static,
) -> Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = SETTINGS_WRITE_LOCK
            .lock()
            .map_err(|_| anyhow!("settings write lock poisoned"))?;
        let signal = app
            .try_state::<RevisionSignal<Settings>>()
            .context("settings not initialized")?;
        let mut next = signal.current().state;
        edit(&mut next);
        settings::save_to_file(&next)?;
        signal.notify(next);
        Ok(())
    })
    .await
    .context("settings save task failed")?
}
