extern crate alloc;

use alloc::sync::Arc;
use anyhow::{Context as _, Result, anyhow};
use log::LevelFilter;
use std::sync::Mutex;
use tauri::{Manager as _, WebviewWindowBuilder};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

mod agents;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod autostart;
mod logs;
mod proxy;
mod revision_signal;
mod settings;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray;
mod updater;

const MAIN_WINDOW_NAME: &str = "main";
static MAIN_WINDOW_LOCK: Mutex<()> = Mutex::new(());

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

    let minimized = std::env::args_os().skip(1).any(|arg| arg == "--minimized");

    let app = tauri::Builder::default()
        // This plugin must be registered first to stop a second process before other plugins start.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().skip(1).any(|arg| arg == "--minimized") {
                request_main_window(app);
            }
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
        .manage(proxy::ProxyState::default())
        .manage(revision_signal::RevisionSignal::new(proxy::Proxy::default()))
        .manage(Arc::new(revision_signal::RevisionSignal::new(
            updater::UpdateState::Checking,
        )))
        .manage(revision_signal::RevisionSignal::new(
            agents::AgentRevisionState,
        ))
        .setup(move |app| {
            log::info!("----------");
            log::info!("starting CoreInfra Wizard {}", app.package_info().version);
            tauri::async_runtime::block_on(settings::initialize_state(app.handle()))?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_enabled = {
                let proxy_enabled = settings::get_state(app.handle())?.local_proxy_enabled();
                autostart::initialize(app.handle())?;
                autostart::set_enabled(app.handle(), proxy_enabled)?;

                app.on_menu_event(|app, event| tray::on_menu_event(app, &event));
                tray::set_enabled(app.handle(), proxy_enabled)?;
                tray::is_enabled(app.handle())
            };
            #[cfg(target_os = "linux")]
            let tray_enabled = false;

            if let Err(error) = tauri::async_runtime::block_on(proxy::sync(app.handle())) {
                log::error!("proxy startup failed: {error:#}");
            }

            if !minimized || !tray_enabled {
                request_main_window(app.handle());
            }
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
            proxy::get_proxy_state,
            agents::get_agent_state,
            agents::get_agent_backups,
            agents::prepare_agent_change,
            agents::apply_agent_plan,
            agents::apply_direct_change,
            agents::wait_for_update,
        ])
        .build(tauri::generate_context!())
        .context("failed to build CoreInfra Wizard")?;

    app.run(|app, event| match event {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } if tray::is_enabled(app) && app.webview_windows().is_empty() => {
            // Keep the process, not the closed window. Explicit tray exit uses code Some(0).
            api.prevent_exit();
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => request_main_window(app),
        tauri::RunEvent::Exit => {
            if let Err(error) = tauri::async_runtime::block_on(proxy::stop(app)) {
                log::error!("proxy shutdown failed: {error:#}");
            }
        }
        _ => {}
    });
    Ok(())
}

fn request_main_window(app: &tauri::AppHandle) {
    let app = app.clone();
    // WebView2 creation must not block a synchronous event callback on Windows.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = open_main_window(&app) {
            log::error!("failed to open main window: {error:#}");
        }
    });
}

fn open_main_window(app: &tauri::AppHandle) -> Result<()> {
    let _guard = MAIN_WINDOW_LOCK
        .lock()
        .map_err(|_| anyhow!("main window lock poisoned"))?;
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW_NAME)
        .context("main window configuration not found")?;
    let window = match app.get_webview_window(MAIN_WINDOW_NAME) {
        Some(window) => window,
        None => WebviewWindowBuilder::from_config(app, config)?.build()?,
    };

    window.set_title(&format!("{} v{}", config.title, app.package_info().version))?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}
