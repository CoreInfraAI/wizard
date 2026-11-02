//! Persisted daemon demand and independently observed OS state, shared by settings and tray.
//! Only explicit user operations change OS settings; observers never reconcile them.

use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use anyhow::{Context as _, Result, bail, ensure};
use core::time::Duration;
use serde::Serialize;
use std::sync::Mutex;
use tauri::Manager as _;
use wizard_system_proxy::{self as os, ProxyState, SystemProxyStatus};

static OPERATION: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    #[default]
    Unknown,
    Off,
    On,
    Partial,
    Unavailable,
    Unsupported,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent observed facts and persisted demand, not mutually exclusive states"
)]
pub(crate) struct State {
    pub(crate) status: Status,
    pub(crate) busy: bool,
    pub(crate) conflict: bool,
    pub(crate) listener_running: bool,
    pub(crate) requested: bool,
    read_error: Option<String>,
    operation_error: Option<String>,
}

#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    expect(dead_code)
)]
impl State {
    pub(crate) fn has_ours(&self) -> bool {
        matches!(self.status, Status::On | Status::Partial)
    }

    pub(crate) fn can_toggle(&self) -> bool {
        !self.busy
            && self.status != Status::Unsupported
            && (self.requested || self.has_ours() || (self.status == Status::Off && !self.conflict))
    }
}

pub(crate) fn current(app: &tauri::AppHandle) -> State {
    app.state::<RevisionSignal<State>>().current().state
}

fn update(signal: &RevisionSignal<State>, edit: impl FnOnce(&mut State)) {
    signal.update(|current| {
        let mut next = current.clone();
        edit(&mut next);
        (next != *current).then_some(next)
    });
}

fn begin(signal: &RevisionSignal<State>) -> Result<(), String> {
    signal
        .update(|current| {
            (!current.busy).then(|| State {
                busy: true,
                operation_error: None,
                ..current.clone()
            })
        })
        .map(|_| ())
        .ok_or_else(|| "Изменение системного прокси уже выполняется".into())
}

fn observe(state: &mut State, result: Result<SystemProxyStatus>, listener_running: bool) {
    state.listener_running = listener_running;
    state.conflict = false;
    let previous_error = state.read_error.take();
    match result {
        Ok(snapshot) => {
            if previous_error.is_some() {
                log::info!("system proxy observation recovered");
            }
            state.conflict = snapshot.services.iter().any(|s| s.conflict);
            let any = snapshot
                .services
                .iter()
                .any(|s| s.http == ProxyState::Wizard || s.https == ProxyState::Wizard);
            let all = snapshot
                .services
                .iter()
                .all(|s| s.http == ProxyState::Wizard && s.https == ProxyState::Wizard);
            state.status = if snapshot.services.is_empty() {
                Status::Unavailable
            } else if all && !state.conflict {
                Status::On
            } else if any {
                Status::Partial
            } else {
                Status::Off
            };
        }
        Err(error) => {
            state.status = Status::Unknown;
            let message = format!("Не удалось прочитать системный прокси: {error:#}");
            if previous_error.as_ref() != Some(&message) {
                // OS backend errors contain static diagnostics and numeric native codes,
                // never proxy snapshots, addresses or PAC URLs.
                log::error!("system proxy observation failed: {error:#}");
            }
            state.read_error = Some(message);
        }
    }
}

fn sync_tray(app: &tauri::AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if crate::tray::sync_system_proxy(app).is_err() {
        log::error!("failed to update system proxy tray item");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = app;
}

fn observe_current(app: &tauri::AppHandle) -> Result<State> {
    let requested = crate::settings::get_state(app)?.system_proxy_enabled;
    let status = os::status();
    update(&app.state::<RevisionSignal<State>>(), |state| {
        state.requested = requested;
        observe(state, status, crate::proxy::running(app));
    });
    Ok(current(app))
}

fn os_errors(write: Result<()>, observed: &State, enabled: bool) -> Vec<String> {
    let written = write.is_ok();
    let mut errors = Vec::new();
    if let Err(error) = write {
        // Only errors from wizard-system-proxy: its diagnostics never include settings.
        log::error!("system proxy OS update failed: {error:#}");
        if error.is::<os::AuthorizationRequired>() {
            errors.push("macOS не разрешила изменение системного прокси. Подтвердите запрос администратора и повторите попытку. Настройки ОС не изменены.".into());
        } else {
            errors.push(format!("Не удалось изменить системный прокси: {error:#}"));
        }
    }
    if let Some(error) = &observed.read_error {
        errors.push(error.clone());
    } else if written && observed.status != if enabled { Status::On } else { Status::Off } {
        log::error!("system proxy OS state does not match the requested state");
        errors.push("Операция завершилась, но состояние системного прокси не соответствует запрошенному. Возможно, настройки изменены другой программой.".into());
    }
    errors
}

fn log_operation_result(enabled: bool, forced: bool, result: &Result<(), String>) {
    if result.is_ok() {
        log::info!("system proxy operation completed: enabled={enabled}, forced={forced}");
    } else {
        // The full GUI error may originate in Hub or settings I/O. Do not log it.
        // Safe OS-specific causes are logged at their source instead.
        log::error!("system proxy operation failed: enabled={enabled}, forced={forced}");
    }
}

fn apply(app: &tauri::AppHandle, enabled: bool, forced: bool) -> Result<()> {
    // Token acquisition may update the session; never hold the settings lock here.
    let token = if enabled {
        log::info!("preparing system proxy token and listener");
        Some(
            tauri::async_runtime::block_on(crate::hub_tokens::proxy_token(app))
                .map_err(anyhow::Error::msg)?,
        )
    } else {
        None
    };
    let guard = crate::settings::lock_state()?;
    let previous = crate::settings::get_state(app)?;
    if let Some(token) = token {
        ensure!(
            previous.hub_session == token.session,
            "Сессия CoreInfra Hub изменилась; повторите применение"
        );
        crate::settings::update_state_locked(app, &guard, |settings| {
            settings.proxy_token = Some(token.value);
            settings.system_proxy_enabled = true;
        })?;
        let started = tauri::async_runtime::block_on(crate::proxy::sync(app)).and_then(|()| {
            ensure!(
                crate::proxy::running(app),
                "Локальный прокси не подтвердил готовность"
            );
            Ok(())
        });
        if let Err(error) = started {
            // Startup can time out after taking effect. Restore demand, but never
            // shut down a listener that the OS may already be using.
            let observed = observe_current(app)?;
            let keep = previous.system_proxy_enabled || observed.status != Status::Off;
            crate::settings::update_state_locked(app, &guard, |settings| {
                settings.system_proxy_enabled = keep;
            })
            .with_context(|| {
                format!("Не удалось запустить прокси: {error:#}; не удалось восстановить спрос")
            })?;
            if !keep {
                tauri::async_runtime::block_on(crate::proxy::sync(app))
                    .with_context(|| format!("Не удалось запустить прокси: {error:#}; не удалось синхронизировать прежний спрос"))?;
            }
            return Err(error)
                .context("Системные настройки не изменены: локальный прокси не готов");
        }
    }

    log::info!("applying system proxy OS settings");
    let write = if enabled {
        os::enable(forced)
    } else {
        os::disable()
    };
    let observed = observe_current(app)?; // Also after partial writes or failed verification.
    let mut errors = os_errors(write, &observed, enabled);
    let demand = if observed.status == Status::Off {
        enabled && previous.system_proxy_enabled
    } else {
        true // Partial, unknown, or unavailable: the OS may still reference us.
    };
    let saved = crate::settings::update_state_locked(app, &guard, |settings| {
        settings.system_proxy_enabled = demand;
    });
    // Do not stop the daemon when saving the safety demand failed.
    let synced = if saved.is_ok() && (!enabled || !demand) {
        tauri::async_runtime::block_on(crate::proxy::sync(app))
    } else {
        Ok(())
    };
    errors.extend(
        [
            saved.err().map(|error| {
                format!("Не удалось сохранить состояние системного прокси в Wizard: {error:#}")
            }),
            synced
                .err()
                .map(|error| format!("Не удалось синхронизировать локальный прокси: {error:#}")),
        ]
        .into_iter()
        .flatten(),
    );
    if !errors.is_empty() {
        bail!(errors.join("; "));
    }
    Ok(())
}

async fn run(app: tauri::AppHandle, enabled: Option<bool>, forced: bool) -> Result<(), String> {
    if let Some(enabled) = enabled {
        log::info!("system proxy operation requested: enabled={enabled}, forced={forced}");
    }
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let signal = worker_app.state::<RevisionSignal<State>>();
        if enabled.is_some() {
            begin(&signal).inspect_err(|_| {
                log::warn!("system proxy operation rejected: another operation is in progress");
            })?;
            sync_tray(&worker_app);
        }
        let result = (|| {
            // Mutations wait for a preceding read. Observations skip busy work.
            let _guard = if enabled.is_some() {
                OPERATION.lock().map_err(|_| {
                    "Операции системного прокси недоступны; перезапустите Wizard".to_owned()
                })?
            } else {
                let Ok(guard) = OPERATION.try_lock() else {
                    return Ok(());
                };
                if signal.current().state.busy {
                    return Ok(());
                }
                guard
            };
            if let Some(enabled) = enabled {
                apply(&worker_app, enabled, forced).map_err(|error| format!("{error:#}"))
            } else {
                let state = observe_current(&worker_app).map_err(|error| format!("{error:#}"))?;
                state.read_error.map_or(Ok(()), Err)
            }
        })();
        if let Some(enabled) = enabled {
            // Keep outcome logging in the worker even if its IPC caller disappears.
            log_operation_result(enabled, forced, &result);
            update(&signal, |state| {
                state.busy = false;
                state.listener_running = crate::proxy::running(&worker_app);
                if let Ok(settings) = crate::settings::get_state(&worker_app) {
                    state.requested = settings.system_proxy_enabled;
                }
                if let Err(error) = &result {
                    state.operation_error = Some(error.clone());
                }
            });
        }
        sync_tray(&worker_app);
        result
    })
    .await;
    // Publication stays inside the worker, even if the IPC caller disappears.
    result.unwrap_or_else(|_| {
        log::error!("system proxy worker interrupted; OS state unknown");
        let message = "Операция системного прокси прервана. Состояние ОС неизвестно.";
        update(&app.state::<RevisionSignal<State>>(), |state| {
            if enabled.is_some() {
                state.busy = false;
            }
            state.status = Status::Unknown;
            state.read_error = Some(message.into());
        });
        sync_tray(&app);
        Err(message.into())
    })
}

struct Observer(tauri::async_runtime::JoinHandle<()>);
impl Drop for Observer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) fn start(app: &tauri::AppHandle) {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        update(&app.state::<RevisionSignal<State>>(), |state| {
            state.status = Status::Unsupported;
        });
        return;
    }
    let worker_app = app.clone();
    app.manage(Observer(tauri::async_runtime::spawn(async move {
        loop {
            let _ = run(worker_app.clone(), None, false).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })));
}

pub(crate) fn stop(app: &tauri::AppHandle) {
    if let Some(observer) = app.try_state::<Observer>() {
        observer.0.abort();
    }
}

#[tauri::command]
pub(crate) async fn get_system_proxy_state(
    app: tauri::AppHandle,
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<State>>,
) -> Result<RevisionSnapshot<State>, String> {
    // The settings page starts each subscription with no revision. Unlike the
    // periodic observer, wait for any current operation before reading fresh state.
    if last_revision.is_none() && cfg!(any(target_os = "macos", target_os = "windows")) {
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = OPERATION.lock().map_err(|_| {
                "Операции системного прокси недоступны; перезапустите Wizard".to_owned()
            })?;
            tauri::async_runtime::block_on(crate::proxy::observe(&app));
            observe_current(&app).map_err(|error| format!("{error:#}"))?;
            sync_tray(&app);
            Ok::<(), String>(())
        })
        .await
        .map_err(|_| "Не удалось обновить состояние системного прокси")??;
    }
    signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить состояние системного прокси".into())
}

#[tauri::command]
pub(crate) async fn set_system_proxy(
    app: tauri::AppHandle,
    enabled: bool,
    forced: bool,
) -> Result<(), String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return Err("Системный прокси пока доступен только на macOS и Windows".into());
    }
    run(app, Some(enabled), forced).await
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn toggle_from_tray(app: &tauri::AppHandle) {
    let state = current(app);
    if !state.can_toggle() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if set_system_proxy(app.clone(), !(state.requested || state.has_ours()), false)
            .await
            .is_err()
        {
            crate::request_settings_window(&app);
        }
    });
}

#[cfg(test)]
mod tests;
