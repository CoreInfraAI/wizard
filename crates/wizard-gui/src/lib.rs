extern crate alloc;

use alloc::sync::Arc;
use anyhow::{Context as _, Result};
use log::LevelFilter;
use tauri::Manager as _;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

mod agents;
mod logs;
mod revision_signal;
mod settings;
mod updater;

const MAIN_WINDOW_NAME: &str = "main";

pub fn run_application() -> Result<()> {
    let log_level = match std::env::var("WIZARD_LOG") {
        Ok(value) => value
            .parse::<LevelFilter>()
            .context("invalid WIZARD_LOG: expected off, error, warn, info, debug or trace")?,
        Err(std::env::VarError::NotPresent) => {
            // info by default
            LevelFilter::Info
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("WIZARD_LOG must contain valid Unicode");
        }
    };

    tauri::Builder::default()
        // This plugin must be registered first to stop a second process before other plugins start.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            // focus on second execution
            focus_window(app);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir {
                        file_name: Some("Wizard".into()),
                    }),
                ])
                .level(log_level)
                .max_file_size(1024 * 1024)
                .rotation_strategy(RotationStrategy::KeepSome(5))
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(revision_signal::RevisionSignal::new(
            updater::UpdateState::Checking,
        )))
        .manage(revision_signal::RevisionSignal::new(
            agents::AgentRevisionState,
        ))
        .setup(|app| {
            log::info!("----------");
            log::info!("starting CoreInfra Wizard {}", app.package_info().version);
            if let Some(window) = app.get_webview_window(MAIN_WINDOW_NAME) {
                window.set_title(&format!(
                    "{} v{}",
                    window.title()?,
                    app.package_info().version
                ))?;
            }
            tauri::async_runtime::block_on(settings::initialize_state(app.handle()))?;
            // focus after restart
            focus_window(app.handle());
            updater::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            logs::open_logs,
            logs::read_logs,
            updater::get_update_state,
            updater::request_update,
            updater::retry_update_check,
            settings::get_settings_state,
            agents::get_agent_state,
            agents::get_agent_backups,
            agents::agent_event,
            agents::wait_for_update,
        ])
        .run(tauri::generate_context!())
        .context("failed to run CoreInfra Wizard")
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
