use std::sync::Mutex;

use anyhow::{Context as _, Result, anyhow, bail};
use tauri::Manager as _;
use wizard_core::settings::{self, Settings};

pub(crate) async fn initialize_state(app: &tauri::AppHandle) -> Result<()> {
    let settings = tauri::async_runtime::spawn_blocking(settings::load_from_file)
        .await
        .context("settings initialization task failed")??;
    if !app.manage(Mutex::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
}

/// TODO: update state when file changes
pub(crate) async fn get_state(app: &tauri::AppHandle) -> Result<Settings> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<Mutex<Settings>>()
            .context("settings not initialized")?;
        let current = state
            .lock()
            .map_err(|_| anyhow!("settings lock poisoned"))?;
        Ok(current.clone())
    })
    .await
    .context("settings read task failed")?
}

/// on save error still updates state
pub(crate) async fn update_state(
    app: &tauri::AppHandle,
    edit: impl FnOnce(&mut Settings) + Send + 'static,
) -> Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<Mutex<Settings>>()
            .context("settings not initialized")?;
        let mut current = state
            .lock()
            .map_err(|_| anyhow!("settings lock poisoned"))?;
        edit(&mut current);
        settings::save_to_file(&current)?;
        Ok(())
    })
    .await
    .context("settings save task failed")?
}
