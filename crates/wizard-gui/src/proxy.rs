use crate::settings;
use anyhow::{Context as _, Result};
use tauri::Manager as _;
use tokio::sync::Mutex;
use wizard_proxy::Proxy;

#[derive(Default)]
pub(crate) struct ProxyState(Mutex<Option<Proxy>>);

/// Called at startup and after a persisted proxy flag change, not for token-only edits.
pub(crate) async fn sync(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut proxy = state.0.lock().await;
    let settings = settings::get_state(app)?;

    if settings.claude_proxy_enable {
        if proxy.is_none() {
            // TODO: Retry failed starts without requiring a toggle or application restart.
            *proxy = Some(
                wizard_proxy::start(&settings.coreinfra_token)
                    .await
                    .context("failed to start local proxy")?,
            );
            log::info!("local proxy started on {}", wizard_proxy::LISTEN_ADDRESS);
        }
    } else {
        stop_proxy(&mut proxy).await?;
    }
    Ok(())
}

pub(crate) async fn stop(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut proxy = state.0.lock().await;
    stop_proxy(&mut proxy).await
}

async fn stop_proxy(proxy: &mut Option<Proxy>) -> Result<()> {
    if let Some(proxy) = proxy.take() {
        proxy.stop().await.context("failed to stop local proxy")?;
        log::info!("local proxy stopped");
    }
    Ok(())
}
