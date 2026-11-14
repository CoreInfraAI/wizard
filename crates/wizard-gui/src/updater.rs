use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use alloc::sync::Arc;
use anyhow::{Context as _, Result, anyhow};
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use serde::Serialize;
use std::time::Instant;
use tauri::Manager as _;
use tauri::http::{HeaderValue, header::COOKIE};
use tauri_plugin_updater::{Update, UpdaterExt as _};
use wizard_core::hub::{HUB_ORIGIN, Url};

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_hours(1);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum UpdateState {
    Checking,
    Installing,
    UpToDate,
    Available { version: String },
    Failed { message: String },
}

impl UpdateState {
    fn is_in_progress(&self) -> bool {
        matches!(self, Self::Checking | Self::Installing)
    }
}

/// The updater's configured endpoints, tried in order. Hub's endpoint also receives the session
/// and Wizard's usage parameters; the others are fallbacks that receive neither.
struct UpdateSources {
    endpoints: Vec<Endpoint>,
    started_at: Instant,
    /// Kept until Hub answers a check, so a failed first check reports it again.
    first_run: AtomicBool,
}

struct Endpoint {
    url: Url,
    hub: bool,
}

impl UpdateSources {
    fn from_config(app: &tauri::AppHandle, first_run: bool) -> Result<Self> {
        let config = app
            .config()
            .plugins
            .0
            .get("updater")
            .cloned()
            .context("updater is not configured")?;
        let config: tauri_plugin_updater::Config =
            serde_json::from_value(config).context("invalid updater configuration")?;
        let hub_origin = Url::parse(HUB_ORIGIN)?.origin();
        let endpoints = config
            .endpoints
            .into_iter()
            .map(|url| Endpoint {
                hub: url.origin() == hub_origin,
                url,
            })
            .collect();
        Ok(Self {
            endpoints,
            started_at: Instant::now(),
            first_run: AtomicBool::new(first_run),
        })
    }
}

pub(crate) fn start(app: tauri::AppHandle, first_run: bool) -> Result<()> {
    let sources = UpdateSources::from_config(&app, first_run)?;
    let disabled = sources.endpoints.is_empty();
    app.manage(sources);
    if disabled {
        log::info!("application updates disabled: no endpoints configured");
        app.state::<Arc<RevisionSignal<UpdateState>>>()
            .notify(UpdateState::UpToDate);
        return Ok(());
    }
    start_update(app.clone());

    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
            check_in_background(&app, &state).await;
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_update_state(
    last_revision: Option<u32>,
    state: tauri::State<'_, Arc<RevisionSignal<UpdateState>>>,
) -> Result<RevisionSnapshot<UpdateState>, String> {
    // state.update(|current| {
    //     (!current.is_in_progress()).then(|| UpdateState::Failed { message: "test update error".to_owned() })
    //     // (!current.is_in_progress()).then(|| UpdateState::Available { version: "0.0.0".to_owned() })
    // });
    state
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub(crate) fn request_update(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    // Claim the next update atomically to prevent concurrent installations.
    let Some(_) = state.update(|current| {
        if current.is_in_progress() {
            return None;
        }
        Some(UpdateState::Checking)
    }) else {
        return;
    };
    start_update(app);
}

#[tauri::command]
pub(crate) fn retry_update_check(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    if !matches!(state.current().state, UpdateState::Failed { .. }) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        check_in_background(&app, &state).await;
    });
}

fn start_update(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    tauri::async_runtime::spawn(async move {
        if let Err(error) = check_and_install_update(&app, &state).await {
            log::error!("application update failed: {error:#}");
            state.notify(UpdateState::Failed {
                message: format!("{error:#}"),
            });
        }
    });
}

async fn check_and_install_update(
    app: &tauri::AppHandle,
    state: &RevisionSignal<UpdateState>,
) -> Result<()> {
    log::info!("checking for application updates");

    let Some(mut update) = find_update(app).await? else {
        log::info!("application is up to date");
        state.notify(UpdateState::UpToDate);
        return Ok(());
    };
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);

    log::info!("downloading application update {}", update.version);
    state.notify(UpdateState::Installing);

    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .context("Не удалось скачать обновление")?;

    // download() verifies the update signature before service removal begins.
    let update_guard = wizard_proxy_daemon::prepare_update()
        .await
        .context("Не удалось подготовить прокси к обновлению; установка отменена")?;

    let (update_guard, installation) = tauri::async_runtime::spawn_blocking(move || {
        // Keep lifecycle locked even if the awaiting task is cancelled while the
        // blocking installer runs. Catch installer panics so recovery retains the guard.
        let installation =
            std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| update.install(bytes)))
                .map_err(|_| anyhow::anyhow!("Задача установки обновления завершилась с panic"))
                .and_then(|result| result.context("Не удалось установить обновление"));
        (update_guard, installation)
    })
    .await
    .context("Не удалось выполнить задачу установки обновления")?;

    if let Err(error) = installation {
        if let Err(recovery) = update_guard.restore_after_failure().await {
            return Err(error.context(format!(
                "Также не удалось восстановить сервис прокси: {recovery}"
            )));
        }
        return Err(error);
    }

    // Keep update_guard until process restart; queued lifecycle calls must not
    // start a daemon from the new bundle while the old GUI is still running.
    log::info!("application update installed; restarting");
    app.restart();
}

async fn check_in_background(app: &tauri::AppHandle, state: &RevisionSignal<UpdateState>) {
    log::info!("checking for application updates in background");
    let next = match find_update(app).await {
        Ok(Some(update)) => {
            log::info!("application update {} is available", update.version);
            UpdateState::Available {
                version: update.version,
            }
        }
        Ok(None) => UpdateState::UpToDate,
        Err(error) => {
            log::error!("background update check failed: {error:#}");
            return;
        }
    };
    // Do not overwrite an installation request accepted during the network check.
    state.update(|current| {
        if current.is_in_progress() || current == &next {
            return None;
        }
        Some(next)
    });
}

async fn find_update(app: &tauri::AppHandle) -> Result<Option<Update>> {
    let sources = app.state::<UpdateSources>();
    // An empty endpoint list explicitly disables updates for local test bundles.
    if sources.endpoints.is_empty() {
        return Ok(None);
    }
    let mut last_error = None;
    for endpoint in &sources.endpoints {
        let result = if endpoint.hub {
            check_via_hub(app, &sources, endpoint.url.clone()).await
        } else {
            check(app, endpoint.url.clone(), None).await
        };
        match result {
            Ok(update) => return Ok(update),
            Err(error) => {
                log::warn!(
                    "update check via {} failed: {error:#}",
                    endpoint.url.host_str().unwrap_or_default()
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("no update endpoints configured")))
        .context("Не удалось проверить обновления")
}

async fn check_via_hub(
    app: &tauri::AppHandle,
    sources: &UpdateSources,
    endpoint: Url,
) -> Result<Option<Update>> {
    let settings = crate::settings::get_state(app)?;
    let endpoint = with_usage(
        endpoint,
        &settings.install_id,
        sources.started_at.elapsed(),
        sources.first_run.load(Ordering::Relaxed),
    );
    let mut update = check(app, endpoint, session_cookie(&settings.hub_session)?).await?;
    sources.first_run.store(false, Ordering::Relaxed);
    if let Some(update) = &mut update {
        // The bundle downloads from GitHub; the session is only for Hub.
        update.headers.remove(COOKIE);
    }
    Ok(update)
}

fn with_usage(mut endpoint: Url, install_id: &str, uptime: Duration, first_run: bool) -> Url {
    {
        let mut query = endpoint.query_pairs_mut();
        query
            .append_pair("install_id", install_id)
            .append_pair("uptime", &uptime.as_secs().to_string());
        if first_run {
            query.append_pair("first_run", "1");
        }
    }
    endpoint
}

fn session_cookie(session: &str) -> Result<Option<HeaderValue>> {
    if session.is_empty() {
        return Ok(None);
    }
    let mut cookie = HeaderValue::from_str(&format!("session={session}"))
        .map_err(|_| anyhow!("Hub session is not a valid cookie value"))?;
    cookie.set_sensitive(true);
    Ok(Some(cookie))
}

async fn check(
    app: &tauri::AppHandle,
    endpoint: Url,
    session: Option<HeaderValue>,
) -> Result<Option<Update>> {
    let mut builder = app
        .updater_builder()
        // Client-level, so it also covers proxy CONNECT and the download.
        .configure_client(|client| client.user_agent(wizard_proxy::user_agent()))
        .timeout(UPDATE_CHECK_TIMEOUT)
        .endpoints(vec![endpoint])
        .context("Не удалось подготовить проверку обновлений")?;
    if let Some(session) = session {
        builder = builder
            .header(COOKIE, session)
            .context("Не удалось подготовить проверку обновлений")?;
    }
    let updater = builder
        .build()
        .context("Не удалось подготовить проверку обновлений")?;
    Ok(updater.check().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_endpoint_keeps_channel_and_adds_usage() {
        let endpoint = Url::parse("https://hub.coreinfra.ai/hub/api/wizard/release?channel=dev")
            .expect("valid endpoint");
        let url = with_usage(
            endpoint.clone(),
            "abcdefgh27",
            Duration::from_secs(90),
            true,
        );
        assert_eq!(
            url.as_str(),
            "https://hub.coreinfra.ai/hub/api/wizard/release?channel=dev&install_id=abcdefgh27&uptime=90&first_run=1"
        );
        let url = with_usage(endpoint, "abcdefgh27", Duration::from_millis(999), false);
        assert_eq!(
            url.query(),
            Some("channel=dev&install_id=abcdefgh27&uptime=0")
        );
    }
}
