use anyhow::{Context as _, Result};
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::TrayIconBuilder;

const TRAY_ID: &str = "main";
const OPEN_ID: &str = "wizard-open";
const EXIT_ID: &str = "wizard-exit";

pub(crate) fn on_menu_event(app: &tauri::AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        OPEN_ID => crate::request_main_window(app),
        EXIT_ID => app.exit(0),
        _ => {}
    }
}

pub(crate) fn is_enabled(app: &tauri::AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

/// Called at startup or under the serialized settings writer.
pub(crate) fn set_enabled(app: &tauri::AppHandle, enabled: bool) -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = sender.send(set_enabled_on_main_thread(&handle, enabled));
    })
    .context("failed to dispatch tray update")?;
    receiver.recv().context("tray update did not complete")?
}

fn set_enabled_on_main_thread(app: &tauri::AppHandle, enabled: bool) -> Result<()> {
    // Removing the registry entry drops the native icon; AppKit requires the main thread.
    if !enabled {
        // The settings writer decides whether to exit after all OS effects finish.
        app.remove_tray_by_id(TRAY_ID);
        return Ok(());
    }
    if is_enabled(app) {
        return Ok(());
    }

    let open = MenuItem::with_id(app, OPEN_ID, "Открыть Wizard", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, EXIT_ID, "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &exit])?;
    let icon = app
        .default_window_icon()
        .context("application icon is missing")?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon.clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)
        .context("failed to create Wizard tray icon")?;
    Ok(())
}
