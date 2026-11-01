use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use anyhow::{Context as _, Result, ensure};
use core::time::Duration;
use serde::Serialize;
use std::{io, sync::OnceLock};
use tauri::Manager as _;
use tokio::sync::Mutex;

/// Runtime proxy enablement reported by the daemon, not saved demand.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Proxy {
    enabled: bool,
}

pub(crate) fn running(app: &tauri::AppHandle) -> bool {
    // Report the observed daemon state, not saved demand or a GUI-owned listener.
    // Unknown status must not be presented as a working proxy.
    app.state::<ProxyState>()
        .signal
        .current()
        .state
        .is_ok_and(|proxy| proxy.enabled)
}

pub(crate) struct ProxyState {
    // Serialize observations with sync so an earlier status cannot overwrite a
    // status fetched after Reload. This does not own the daemon's lifecycle.
    operation: Mutex<()>,
    signal: RevisionSignal<Result<Proxy, String>>,
    observer: OnceLock<tauri::async_runtime::JoinHandle<()>>,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self {
            operation: Mutex::new(()),
            signal: RevisionSignal::new(Err("proxy status is not loaded".to_owned())),
            observer: OnceLock::new(),
        }
    }
}

#[tauri::command]
pub(crate) async fn get_proxy_state(
    last_revision: Option<u32>,
    state: tauri::State<'_, ProxyState>,
) -> Result<RevisionSnapshot<Proxy>, String> {
    let snapshot = state
        .signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))?;
    Ok(RevisionSnapshot {
        revision: snapshot.revision,
        state: snapshot.state?,
    })
}

/// Startup only: provision the shared credential for existing demand, including
/// migrated settings. An existing nonempty credential never triggers Hub requests.
pub(crate) async fn prepare_token(app: &tauri::AppHandle) -> Result<()> {
    let current = crate::settings::get_state(app)?;
    if !current.local_proxy_enabled()
        || current
            .proxy_token
            .as_deref()
            .is_some_and(|token| !token.is_empty())
    {
        return Ok(());
    }
    // Hub requests may save a refreshed session; acquire the settings lock later.
    let token = crate::hub_tokens::proxy_token(app)
        .await
        .map_err(anyhow::Error::msg)?;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = crate::settings::lock_state()?;
        let current = crate::settings::get_state(&app)?;
        if !current.local_proxy_enabled()
            || current
                .proxy_token
                .as_deref()
                .is_some_and(|token| !token.is_empty())
        {
            return Ok(());
        }
        ensure!(
            current.hub_session == token.session,
            "Сессия CoreInfra Hub изменилась; повторите применение"
        );
        crate::settings::update_state_locked(&app, &guard, |settings| {
            settings.proxy_token = Some(token.value);
        })
    })
    .await
    .context("proxy startup token save task failed")?
}

/// Called at startup and after proxy settings have been saved and published.
pub(crate) async fn sync(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let _guard = state.operation.lock().await;
    let result = wizard_proxy_daemon::sync_proxy()
        .await
        .context("failed to synchronize proxy daemon");
    // Reload can fail; observe runtime instead of assuming
    // it matches the saved settings. Observations never launch or reload anything.
    refresh(&state).await;
    result
}

async fn refresh(state: &ProxyState) {
    let next = match wizard_proxy::control::status().await {
        Ok(status) => Ok(Proxy {
            enabled: status.enabled,
        }),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Ok(Proxy::default()),
        Err(_) => Err("failed to read proxy daemon status".to_owned()),
    };
    state
        .signal
        .update(|current| (current != &next).then_some(next));
}

/// Read runtime state without starting the daemon or applying saved settings.
pub(crate) async fn observe(app: &tauri::AppHandle) {
    let state = app.state::<ProxyState>();
    let _guard = state.operation.lock().await;
    refresh(&state).await;
}

/// Observe the independent process even if it stops outside a settings update.
pub(crate) fn start_observer(app: &tauri::AppHandle) {
    let state = app.state::<ProxyState>();
    let app = app.clone();
    state.observer.get_or_init(|| {
        tauri::async_runtime::spawn(async move {
            loop {
                observe(&app).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    });
}

/// GUI exit stops only its observer, never the independent proxy process.
pub(crate) fn stop_observer(app: &tauri::AppHandle) {
    if let Some(observer) = app.state::<ProxyState>().observer.get() {
        observer.abort();
    }
}
