use anyhow::{Context as _, Result};
use std::sync::Mutex;
use tauri::Manager as _;
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::TrayIconBuilder;

const TRAY_ID: &str = "main";
const OPEN_ID: &str = "wizard-open";
const EXIT_ID: &str = "wizard-exit";
const SYSTEM_PROXY_ID: &str = "wizard-system-proxy";
const PROXY_WARNING_ID: &str = "wizard-system-proxy-warning";

struct ProxyMenu {
    menu: Menu<tauri::Wry>,
    action: MenuItem<tauri::Wry>,
    warning: MenuItem<tauri::Wry>,
}

#[derive(Default)]
pub(crate) struct TrayState(Mutex<Option<ProxyMenu>>);

pub(crate) fn on_menu_event(app: &tauri::AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        OPEN_ID => crate::request_main_window(app),
        EXIT_ID => app.exit(0),
        SYSTEM_PROXY_ID => crate::system_proxy::toggle_from_tray(app),
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
        app.state::<TrayState>()
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("tray state lock poisoned"))?
            .take();
        return Ok(());
    }
    if is_enabled(app) {
        return Ok(());
    }

    let open = MenuItem::with_id(app, OPEN_ID, "Открыть Wizard", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, EXIT_ID, "Выход", true, None::<&str>)?;
    let state = crate::system_proxy::current(app);
    let system_proxy = MenuItem::with_id(
        app,
        SYSTEM_PROXY_ID,
        system_proxy_label(&state),
        state.can_toggle(),
        None::<&str>,
    )?;
    let warning = MenuItem::with_id(
        app,
        PROXY_WARNING_ID,
        "Локальный прокси Claude Desktop остановлен",
        false,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&open, &system_proxy, &exit])?;
    let items = ProxyMenu {
        menu,
        action: system_proxy,
        warning,
    };
    update_proxy_menu(&items, &state)?;
    let icon = app
        .default_window_icon()
        .context("application icon is missing")?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon.clone())
        .menu(&items.menu)
        .show_menu_on_left_click(true)
        .build(app)
        .context("failed to create Wizard tray icon")?;
    *app.state::<TrayState>()
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("tray state lock poisoned"))? = Some(items);
    Ok(())
}

fn system_proxy_label(state: &crate::system_proxy::State) -> &'static str {
    use crate::system_proxy::Status;
    if state.busy {
        return "Системный прокси — применение…";
    }
    match state.status {
        Status::Unknown => "Системный прокси — состояние неизвестно",
        Status::Unavailable => "Системный прокси — нет сетевых сервисов",
        Status::Unsupported => "Системный прокси — недоступен",
        Status::On | Status::Partial => "Выключить системный прокси",
        Status::Off => "Включить системный прокси",
    }
}

fn update_proxy_menu(items: &ProxyMenu, state: &crate::system_proxy::State) -> Result<()> {
    let label = system_proxy_label(state);
    if items.action.text()? != label {
        items.action.set_text(label)?;
    }
    if items.action.is_enabled()? != state.can_toggle() {
        items.action.set_enabled(state.can_toggle())?;
    }
    let warning_visible = items.menu.get(PROXY_WARNING_ID).is_some();
    let show_warning = !state.listener_running
        && matches!(
            state.status,
            crate::system_proxy::Status::Off
                | crate::system_proxy::Status::On
                | crate::system_proxy::Status::Partial
        );
    if show_warning && !warning_visible {
        items.menu.insert(&items.warning, 2)?;
    } else if !show_warning && warning_visible {
        items.menu.remove(&items.warning)?;
    }
    Ok(())
}

/// Always read the newest backend state on the main thread, not a captured
/// snapshot that could arrive after a newer operation or tray recreation.
pub(crate) fn sync_system_proxy(app: &tauri::AppHandle) -> Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let state = crate::system_proxy::current(&handle);
            let tray = handle.state::<TrayState>();
            let item = tray
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("tray state lock poisoned"))?;
            if let Some(items) = item.as_ref() {
                update_proxy_menu(items, &state)?;
            }
            Ok(())
        })();
        let _ = sender.send(result);
    })
    .context("failed to dispatch system proxy tray update")?;
    receiver
        .recv()
        .context("system proxy tray update did not complete")?
}
