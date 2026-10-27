use crate::{
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use anyhow::{Context as _, Result, anyhow};
use serde::Serialize;
use tauri::Manager as _;
use tokio::sync::Mutex;
use wizard_settings::Settings;

/// Actual state of each desktop app's listener.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Proxy {
    claude_desktop: bool,
    chatgpt: bool,
}

#[derive(Clone, Copy)]
enum App {
    ClaudeDesktop,
    ChatGpt,
}

impl App {
    const ALL: [Self; 2] = [Self::ClaudeDesktop, Self::ChatGpt];

    fn name(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "Claude Desktop",
            Self::ChatGpt => "ChatGPT",
        }
    }

    fn address(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => wizard_proxy::CLAUDE_DESKTOP_ADDRESS,
            Self::ChatGpt => wizard_proxy::CHATGPT_ADDRESS,
        }
    }

    fn token(self, settings: &Settings) -> Option<&str> {
        match self {
            Self::ClaudeDesktop => settings.claude_proxy_enabled.as_deref(),
            Self::ChatGpt => settings.codex_proxy_enabled.as_deref(),
        }
    }
}

// No Debug: holds the listener's startup token.
struct RunningProxy {
    handle: wizard_proxy::Proxy,
    observer: tauri::async_runtime::JoinHandle<()>,
    token: String,
}

#[derive(Default)]
struct Listeners {
    claude_desktop: Option<RunningProxy>,
    chatgpt: Option<RunningProxy>,
}

impl Listeners {
    fn get(&mut self, app: App) -> &mut Option<RunningProxy> {
        match app {
            App::ClaudeDesktop => &mut self.claude_desktop,
            App::ChatGpt => &mut self.chatgpt,
        }
    }
}

#[derive(Default)]
pub(crate) struct ProxyState(Mutex<Listeners>);

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
fn publish(app: &tauri::AppHandle, proxy_app: App, running: bool) {
    app.state::<RevisionSignal<Proxy>>().update(|current| {
        let mut next = current.clone();
        match proxy_app {
            App::ClaudeDesktop => next.claude_desktop = running,
            App::ChatGpt => next.chatgpt = running,
        }
        (next != *current).then_some(next)
    });
}

/// Called at startup and after a change of either app's proxy setting. Each listener runs while
/// its app has a token and is restarted when that token changes; one failure does not skip the other.
pub(crate) async fn sync(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut listeners = state.0.lock().await;
    let settings = settings::get_state(app)?;

    let mut results = Vec::new();
    for proxy_app in App::ALL {
        let token = proxy_app.token(&settings);
        results.push(sync_app(app, listeners.get(proxy_app), proxy_app, token).await);
    }
    combine(results)
}

async fn sync_app(
    app: &tauri::AppHandle,
    slot: &mut Option<RunningProxy>,
    proxy_app: App,
    token: Option<&str>,
) -> Result<()> {
    if slot.as_ref().map(|running| running.token.as_str()) == token {
        return Ok(());
    }
    stop_proxy(app, slot, proxy_app).await?;
    let Some(token) = token else {
        return Ok(());
    };

    // TODO: Retry failed starts without requiring a toggle or application restart.
    let handle = wizard_proxy::start(proxy_app.address(), token)
        .await
        .with_context(|| format!("failed to start {} local proxy", proxy_app.name()))?;
    let stopped = handle.stopped();
    let observer_app = app.clone();
    let observer = tauri::async_runtime::spawn(async move {
        stopped.await;

        let state = observer_app.state::<ProxyState>();
        let mut listeners = state.0.lock().await;
        if let Some(running) = listeners.get(proxy_app).take() {
            // The task already exited. Join it to report its error and release the handle.
            let result = running.handle.stop().await;
            publish(&observer_app, proxy_app, false);
            match result {
                Ok(()) => log::warn!("{} local proxy stopped unexpectedly", proxy_app.name()),
                Err(error) => log::error!(
                    "{} local proxy stopped unexpectedly: {error:#}",
                    proxy_app.name()
                ),
            }
        }
    });
    *slot = Some(RunningProxy {
        handle,
        observer,
        token: token.to_owned(),
    });
    publish(app, proxy_app, true);
    log::info!(
        "{} local proxy started on {}",
        proxy_app.name(),
        proxy_app.address()
    );
    Ok(())
}

pub(crate) async fn stop(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<ProxyState>();
    let mut listeners = state.0.lock().await;

    let mut results = Vec::new();
    for proxy_app in App::ALL {
        results.push(stop_proxy(app, listeners.get(proxy_app), proxy_app).await);
    }
    combine(results)
}

async fn stop_proxy(
    app: &tauri::AppHandle,
    slot: &mut Option<RunningProxy>,
    proxy_app: App,
) -> Result<()> {
    if let Some(running) = slot.take() {
        // Cancel this instance's observer before a replacement can acquire the mutex.
        running.observer.abort();
        let result = running.handle.stop().await;
        publish(app, proxy_app, false);
        result.with_context(|| format!("failed to stop {} local proxy", proxy_app.name()))?;
        log::info!("{} local proxy stopped", proxy_app.name());
    }
    Ok(())
}

fn combine(results: Vec<Result<()>>) -> Result<()> {
    let mut errors = results.into_iter().filter_map(Result::err);
    let Some(first) = errors.next() else {
        return Ok(());
    };
    Err(errors.fold(first, |all, error| anyhow!("{all:#}; {error:#}")))
}
