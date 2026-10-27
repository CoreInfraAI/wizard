use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
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

#[tauri::command]
pub(crate) async fn get_settings_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<Settings>>,
) -> Result<RevisionSnapshot<Settings>, String> {
    let mut snapshot = signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))?;
    // Clear only the IPC copy; the persisted session remains backend-only.
    snapshot.state.hub_session.clear();
    Ok(snapshot)
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
        let proxy_changed = next.claude_proxy_enabled != current.claude_proxy_enabled
            || next.codex_proxy_enabled != current.codex_proxy_enabled;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let proxy_enabled = next.local_proxy_enabled();

        settings::save_to_file(&next)?;
        signal.notify(next);

        if !proxy_changed {
            return Ok(());
        }
        let proxy_result = tauri::async_runtime::block_on(crate::proxy::sync(&app));
        if let Err(error) = &proxy_result {
            log::error!("proxy update failed: {error:#}");
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if proxy_enabled != current.local_proxy_enabled() {
            crate::tray::set_enabled(&app, proxy_enabled)?;
            crate::autostart::set_enabled(&app, proxy_enabled)?;
        }

        proxy_result
    })
    .await
    .context("settings save task failed")?
}
