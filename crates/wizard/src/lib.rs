extern crate alloc;

use alloc::sync::Arc;
use std::process::ExitCode;

use log::LevelFilter;
use tauri::Manager as _;
use tauri_plugin_log::RotationStrategy;

mod agents;
mod platform;
mod revision_signal;
#[cfg(target_os = "macos")]
mod toml;
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
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(if cfg!(debug_assertions) {
                    LevelFilter::Debug
                } else {
                    LevelFilter::Info
                })
                .max_file_size(1024 * 1024)
                .rotation_strategy(RotationStrategy::KeepSome(5))
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(updater::StartupUpdateState::default()))
        .manage(revision_signal::RevisionSignal::default())
        .setup(|app| {
            log::info!("starting Wizard {}", app.package_info().version);
            focus_window(app.handle());
            updater::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            updater::wait_for_startup_update,
            agents::get_agent_state,
            agents::agent_event,
            revision_signal::wait_for_update,
        ])
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
