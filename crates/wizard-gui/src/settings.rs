use crate::revision_signal::RevisionSignal;
use anyhow::{Context as _, Result, anyhow, bail};
use std::sync::Mutex;
use tauri::Manager as _;
use wizard_settings::{self as settings, Settings};

static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) async fn initialize_state(app: &tauri::AppHandle) -> Result<()> {
    let settings = tauri::async_runtime::spawn_blocking(settings::load_from_file)
        .await
        .context("settings initialization task failed")??;

    if !app.manage(RevisionSignal::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
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
        let current = signal.current().state;
        let mut next = current.clone();
        edit(&mut next);
        if next == current {
            return Ok(());
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let proxy_enabled = next.local_proxy_enabled();

        settings::save_to_file(&next)?;
        signal.notify(next);

        // Listeners are synced by the caller that changes a proxy setting.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if proxy_enabled != current.local_proxy_enabled() {
            crate::tray::set_enabled(&app, proxy_enabled)?;
            crate::autostart::set_enabled(&app, proxy_enabled)?;
        }
        Ok(())
    })
    .await
    .context("settings save task failed")?
}
