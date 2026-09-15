extern crate alloc;

use alloc::sync::Arc;
use std::process::ExitCode;

use tauri::Manager as _;

mod updater;

const MAIN_WINDOW_NAME: &str = "main";

#[must_use]
pub fn run() -> ExitCode {
    if let Err(error) = run_application() {
        eprintln!("application failed: {error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run_application() -> tauri::Result<()> {
    tauri::Builder::default()
        // This plugin must be registered first to stop a second process before other plugins start.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            focus_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(updater::StartupUpdateState::default()))
        .setup(|app| {
            updater::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![updater::wait_for_startup_update])
        .run(tauri::generate_context!())
}

fn focus_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_NAME) {
        if let Err(error) = window.show() {
            eprintln!("failed to show main window: {error}");
        }
        if let Err(error) = window.unminimize() {
            eprintln!("failed to restore main window: {error}");
        }
        if let Err(error) = window.set_focus() {
            eprintln!("failed to focus main window: {error}");
        }
    }
}
