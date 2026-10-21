use crate::{
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use anyhow::{Context as _, Result};
use serde::Serialize;
use tauri::Manager as _;
use tokio::sync::Mutex;

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Proxy {
    running: bool,
}

struct RunningProxy {
    handle: wizard_proxy::Proxy,
    observer: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub(crate) struct ProxyState(Mutex<Option<RunningProxy>>);

#[tauri::command]
pub(crate) async fn get_proxy_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<Proxy>>,
) -> Result<RevisionSnapshot<Proxy>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

// Call while holding ProxyState's mutex so lifecycle events remain ordered.
fn publish(app: &tauri::AppHandle, running: bool) {
    app.state::<RevisionSignal<Proxy>>()
        .update(|current| (current.running != running).then_some(Proxy { running }));
}

/// Called at startup and after a change of combined proxy demand, not for token-only edits.
pub(crate) async fn sync(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut proxy = state.0.lock().await;
    let settings = settings::get_state(app)?;

    if let Some(token) = settings.local_proxy_token() {
        if proxy.is_none() {
            // TODO: Retry failed starts without requiring a toggle or application restart.
            let handle = wizard_proxy::start(token)
                .await
                .context("failed to start local proxy")?;
            let stopped = handle.stopped();
            let observer_app = app.clone();
            let observer = tauri::async_runtime::spawn(async move {
                stopped.await;

                let state = observer_app.state::<ProxyState>();
                let mut proxy = state.0.lock().await;
                if let Some(running) = proxy.take() {
                    // The task already exited. Join it to report its error and release the handle.
                    let result = running.handle.stop().await;
                    publish(&observer_app, false);
                    match result {
                        Ok(()) => log::warn!("local proxy stopped unexpectedly"),
                        Err(error) => log::error!("local proxy stopped unexpectedly: {error:#}"),
                    }
                }
            });
            *proxy = Some(RunningProxy { handle, observer });
            publish(app, true);
            log::info!("local proxy started on {}", wizard_proxy::LISTEN_ADDRESS);
        }
    } else {
        stop_proxy(app, &mut proxy).await?;
    }
    Ok(())
}

pub(crate) async fn stop(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut proxy = state.0.lock().await;
    stop_proxy(app, &mut proxy).await
}

async fn stop_proxy(app: &tauri::AppHandle, proxy: &mut Option<RunningProxy>) -> Result<()> {
    if let Some(running) = proxy.take() {
        // Cancel this instance's observer before a replacement can acquire the mutex.
        running.observer.abort();
        let result = running.handle.stop().await;
        publish(app, false);
        result.context("failed to stop local proxy")?;
        log::info!("local proxy stopped");
    }
    Ok(())
}
