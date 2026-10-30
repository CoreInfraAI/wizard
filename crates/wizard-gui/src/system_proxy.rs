//! OS-backed state shared by settings and tray. No saved flag or automatic writes.

use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use anyhow::Result;
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
pub(crate) struct State {
    pub(crate) status: Status,
    pub(crate) busy: bool,
    pub(crate) conflict: bool,
    pub(crate) listener_running: bool,
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
        !self.busy && (self.has_ours() || (self.status == Status::Off && !self.conflict))
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
    state.read_error = None;
    match result {
        Ok(snapshot) => {
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
            state.read_error = Some(format!("Не удалось прочитать системный прокси: {error:#}"));
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

async fn run(app: tauri::AppHandle, enabled: Option<bool>) -> Result<(), String> {
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let signal = worker_app.state::<RevisionSignal<State>>();
        if enabled.is_some() {
            begin(&signal)?;
            sync_tray(&worker_app);
        }
        let result = (|| {
            // Mutations wait for a preceding read. Observations skip busy work.
            let _guard = if enabled.is_some() {
                OPERATION.lock().map_err(|_| "Операции системного прокси недоступны; перезапустите Wizard".to_owned())?
            } else {
                let Ok(guard) = OPERATION.try_lock() else { return Ok(()); };
                if signal.current().state.busy { return Ok(()); }
                guard
            };
            let write = enabled.map(|enabled| if enabled { os::enable() } else { os::disable() });
            let status = os::status(); // Always reread, even after a failed write.
            update(&signal, |state| {
                observe(state, status, crate::proxy::claude_desktop_running(&worker_app));
                if let Some(write) = write {
                    state.operation_error = write.err().map(|error| format!("Не удалось изменить системный прокси: {error:#}. Настройки могли примениться частично."));
                }
            });
            let state = current(&worker_app);
            state.operation_error.or(state.read_error).map_or(Ok(()), Err)
        })();
        if enabled.is_some() {
            update(&signal, |state| {
                state.busy = false;
                if let Err(error) = &result
                    && state.operation_error.is_none() && state.read_error.is_none() {
                    state.operation_error = Some(error.clone());
                }
            });
        }
        sync_tray(&worker_app);
        result
    }).await;
    // Publication stays inside the worker, even if the IPC caller disappears.
    result.unwrap_or_else(|_| {
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
            let _ = run(worker_app.clone(), None).await;
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
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<State>>,
) -> Result<RevisionSnapshot<State>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить состояние системного прокси".into())
}

#[tauri::command]
pub(crate) async fn set_system_proxy(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return Err("Системный прокси пока доступен только на macOS и Windows".into());
    }
    run(app, Some(enabled)).await
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn toggle_from_tray(app: &tauri::AppHandle) {
    let state = current(app);
    if !state.can_toggle() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if set_system_proxy(app.clone(), !state.has_ours())
            .await
            .is_err()
        {
            crate::request_settings_window(&app);
        }
    });
}

#[cfg(test)]
mod tests;
