use anyhow::{Context as _, Result};
use tauri_plugin_autostart::ManagerExt as _;

pub(crate) fn initialize(app: &tauri::AppHandle) -> Result<()> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let builder = tauri_plugin_autostart::Builder::new()
        .app_name(app.config().identifier.clone())
        .arg("--minimized");
    #[cfg(target_os = "macos")]
    let builder = builder.macos_launcher(tauri_plugin_autostart::MacosLauncher::LaunchAgent);

    app.plugin(builder.build())
        .context("failed to initialize autostart")
}

pub(crate) fn set_enabled(app: &tauri::AppHandle, enabled: bool) -> Result<()> {
    let autostart = app.autolaunch();
    if enabled {
        // Refresh the executable path and arguments even if an entry already exists.
        autostart.enable().context("failed to enable autostart")
    } else if autostart
        .is_enabled()
        .context("failed to inspect autostart")?
    {
        autostart.disable().context("failed to disable autostart")
    } else {
        Ok(())
    }
}
