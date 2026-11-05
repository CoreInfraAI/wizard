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
mod desktop_install;
mod hub_auth;
mod hub_tokens;
mod logs;
mod proxy;
mod revision_signal;
mod settings;
mod system_proxy;
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

    // Install ring before any HTTP client exists.
    wizard_core::hub::install_crypto_provider()?;

    let minimized = std::env::args_os().skip(1).any(|arg| arg == "--minimized");
    // Only the initial autostart is minimized. Tauri restart and the Windows
    // updater both use this saved argument list, not the live process arguments.
    let mut restart_env = tauri::Env::default();
    restart_env.args_os.retain(|arg| arg != "--minimized");

    let app = tauri::Builder::default()
        .manage(restart_env)
        // This plugin must be registered first to stop a second process before other plugins start.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().skip(1).any(|arg| arg == "--minimized") {
                request_main_window(app);
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "[{}][{}][{}] {}",
                        chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
                        record.target(),
                        record.level(),
                        message,
                    ));
                })
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::Folder {
                        path: wizard_settings::paths::logs_dir()?,
                        file_name: Some("Wizard".into()),
                    }),
                ])
                .level(log_level)
                // HTTP diagnostics can contain the account login URL or session headers.
                .level_for("reqwest", LevelFilter::Off)
                .level_for("hyper", LevelFilter::Off)
                .level_for("hyper_util", LevelFilter::Off)
                .level_for("h2", LevelFilter::Off)
                .max_file_size(1024 * 1024)
                .rotation_strategy(RotationStrategy::KeepSome(5))
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(desktop_install::InstallManagers::default())
        .manage(proxy::ProxyState::default())
        // Proxy startup can sign out a rejected session, which clears the token list.
        .manage(revision_signal::RevisionSignal::new(
            hub_tokens::HubTokens::default(),
        ))
        .manage(revision_signal::RevisionSignal::new(
            system_proxy::State::default(),
        ))
        .manage(Arc::new(revision_signal::RevisionSignal::new(
            updater::UpdateState::Checking,
        )))
        .manage(revision_signal::RevisionSignal::new(
            agents::AgentRevisionState,
        ))
        .setup(move |app| {
            log::info!("----------");
            log::info!("starting CoreInfra Wizard {}", app.package_info().version);
            let first_run =
                tauri::async_runtime::block_on(settings::initialize_state(app.handle()))?;
            hub_auth::initialize(app.handle());
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            let tray_enabled = {
                app.manage(tray::TrayState::default());
                let preferences = settings::get_state(app.handle())?.application;
                autostart::initialize(app.handle())?;
                let autostart = autostart::set_enabled(app.handle(), preferences.autostart_enabled);

                app.on_menu_event(|app, event| tray::on_menu_event(app, &event));
                let tray = tray::set_enabled(app.handle(), preferences.tray_enabled);
                autostart.and(tray)?;
                tray::is_enabled(app.handle())
            };
            #[cfg(target_os = "linux")]
            let tray_enabled = false;

            if let Err(error) = tauri::async_runtime::block_on(async {
                proxy::prepare_token(app.handle()).await?;
                wizard_proxy_daemon::create_daemon().await?;
                proxy::sync(app.handle()).await
            }) {
                log::error!("proxy startup failed: {error:#}");
            }
            // Proxy startup can briefly interrupt the system proxy used by Hub requests.
            hub_tokens::HubTokensManager::start_loading(app.handle())?;
            proxy::start_observer(app.handle());

            system_proxy::start(app.handle());
            if !minimized || !tray_enabled {
                request_main_window(app.handle());
            }
            updater::start(app.handle().clone(), first_run)?;
            desktop_install::start_background_checks(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_install::get_desktop_install_state,
            desktop_install::check_desktop_release,
            desktop_install::download_desktop_release,
            desktop_install::install_desktop_release,
            system_proxy::get_system_proxy_state,
            system_proxy::set_system_proxy,
            settings::get_application_settings,
            settings::set_application_setting,
            logs::open_logs,
            logs::read_logs,
            updater::get_update_state,
            updater::request_update,
            updater::retry_update_check,
            hub_auth::get_hub_account_state,
            hub_auth::login_hub_account,
            hub_auth::logout_hub_account,
            hub_auth::select_hub,
            hub_auth::open_hub,
            hub_tokens::get_hub_tokens_state,
            hub_tokens::get_configured_token_info,
            hub_tokens::refresh_hub_tokens,
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
            system_proxy::stop(app);
            proxy::stop_observer(app);
        }
        _ => {}
    });
    Ok(())
}

fn request_main_window(app: &tauri::AppHandle) {
    request_window(app, false);
}

#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    expect(dead_code)
)]
fn request_settings_window(app: &tauri::AppHandle) {
    request_window(app, true);
}

fn request_window(app: &tauri::AppHandle, show_settings: bool) {
    let app = app.clone();
    // WebView2 creation must not block a synchronous event callback on Windows.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = open_main_window(&app, show_settings) {
            log::error!("failed to open main window: {error:#}");
        }
    });
}

fn open_main_window(app: &tauri::AppHandle, show_settings: bool) -> Result<()> {
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
    let window = if let Some(window) = app.get_webview_window(MAIN_WINDOW_NAME) {
        if show_settings {
            // Static trusted-local navigation, never interpolate command data.
            window.eval("window.location.hash = '/settings';")?;
        }
        window
    } else {
        let mut builder = WebviewWindowBuilder::from_config(app, config)?;
        if show_settings {
            // Runs before React reads the initial hash, including on a newly
            // recreated WebView. No event can be lost before UI subscription.
            builder = builder.initialization_script("window.location.hash = '/settings';");
        }
        builder.build()?
    };

    window.set_title(&format!("{} v{}", config.title, app.package_info().version))?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}
