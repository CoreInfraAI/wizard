use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use alloc::sync::Arc;
use anyhow::{Context as _, Result, anyhow};
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Instant;
use tauri::Manager as _;
use tauri::http::{HeaderValue, header::COOKIE};
use tauri_plugin_updater::{Update, UpdaterExt as _};
use wizard_core::hub::{HUB_ORIGIN, Url};

const UPDATE_CHECK_INTERVAL: Duration = Duration::from_hours(1);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const UPDATE_DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum UpdateState {
    Checking,
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
    },
    ReadyToInstall {
        version: String,
    },
    Installing {
        version: String,
    },
    UpToDate,
    Available {
        version: String,
    },
    Failed {
        message: String,
        version: Option<String>,
    },
}

impl UpdateState {
    fn is_in_progress(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. } | Self::ReadyToInstall { .. } | Self::Installing { .. }
        )
    }
}

struct DownloadedUpdate {
    update: Update,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct UpdateJob {
    generation: u64,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    ready: Option<DownloadedUpdate>,
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
    app.manage(Mutex::new(UpdateJob::default()));
    if disabled {
        log::info!("application updates disabled: no endpoints configured");
        app.state::<Arc<RevisionSignal<UpdateState>>>()
            .notify(UpdateState::UpToDate);
        return Ok(());
    }
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    tauri::async_runtime::spawn(async move {
        loop {
            check_in_background(&app, &state).await;
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_update_state(
    last_revision: Option<u32>,
    state: tauri::State<'_, Arc<RevisionSignal<UpdateState>>>,
) -> Result<RevisionSnapshot<UpdateState>, String> {
    state
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects application handles by value"
)]
pub(crate) fn request_update(app: tauri::AppHandle) -> Result<(), String> {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    let manager = app.state::<Mutex<UpdateJob>>();
    let mut job = manager.lock().map_err(|_| "Обновление недоступно")?;
    let (UpdateState::Available { version }
    | UpdateState::Failed {
        version: Some(version),
        ..
    }) = state.current().state
    else {
        return Ok(());
    };
    job.generation = job.generation.wrapping_add(1);
    let generation = job.generation;
    state.notify(UpdateState::Downloading {
        version: version.clone(),
        received: 0,
        total: None,
    });
    let application = app.clone();
    job.task = Some(tauri::async_runtime::spawn(async move {
        let result = download_update(&application, &state, generation).await;
        let manager = application.state::<Mutex<UpdateJob>>();
        let Ok(mut job) = manager.lock() else {
            return;
        };
        if job.generation != generation {
            return;
        }
        job.task = None;
        match result {
            Ok(Some(download)) => {
                let version = download.update.version.clone();
                job.ready = Some(download);
                state.notify(UpdateState::ReadyToInstall { version });
            }
            Ok(None) => {
                state.notify(UpdateState::UpToDate);
            }
            Err(error) => {
                log::error!("application update download failed: {error:#}");
                state.notify(UpdateState::Failed {
                    message: format!("{error:#}"),
                    version: Some(version),
                });
            }
        }
    }));
    Ok(())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects application handles by value"
)]
pub(crate) fn cancel_update<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    let state = app.state::<Arc<RevisionSignal<UpdateState>>>();
    let manager = app.state::<Mutex<UpdateJob>>();
    let mut job = manager.lock().map_err(|_| "Обновление недоступно")?;
    let (UpdateState::Downloading { version, .. } | UpdateState::ReadyToInstall { version }) =
        state.current().state
    else {
        return Ok(());
    };
    job.generation = job.generation.wrapping_add(1);
    if let Some(task) = job.task.take() {
        task.abort();
    }
    job.ready = None;
    state.notify(UpdateState::Available { version });
    Ok(())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects application handles by value"
)]
pub(crate) fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    let manager = app.state::<Mutex<UpdateJob>>();
    let mut job = manager.lock().map_err(|_| "Обновление недоступно")?;
    if !matches!(state.current().state, UpdateState::ReadyToInstall { .. }) {
        return Ok(());
    }
    let download = job.ready.take().ok_or("Сначала скачайте обновление")?;
    let version = download.update.version.clone();
    state.notify(UpdateState::Installing {
        version: version.clone(),
    });
    let application = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = install_downloaded_update(&application, download).await {
            log::error!("application update installation failed: {error:#}");
            state.notify(UpdateState::Failed {
                message: format!("{error:#}"),
                version: Some(version),
            });
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn retry_update_check(app: tauri::AppHandle) {
    let state = Arc::clone(&app.state::<Arc<RevisionSignal<UpdateState>>>());
    if !matches!(
        state.current().state,
        UpdateState::Failed { version: None, .. }
    ) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        check_in_background(&app, &state).await;
    });
}

async fn download_update(
    app: &tauri::AppHandle,
    state: &RevisionSignal<UpdateState>,
    generation: u64,
) -> Result<Option<DownloadedUpdate>> {
    let Some(mut update) = find_update(app).await? else {
        return Ok(None);
    };
    update.timeout = Some(UPDATE_DOWNLOAD_TIMEOUT);

    log::info!("downloading application update {}", update.version);
    let mut received = 0;
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                let manager = app.state::<Mutex<UpdateJob>>();
                let Ok(job) = manager.lock() else {
                    return;
                };
                if job.generation == generation {
                    state.notify(UpdateState::Downloading {
                        version: update.version.clone(),
                        received,
                        total,
                    });
                }
            },
            || {},
        )
        .await
        .context("Не удалось скачать обновление")?;
    Ok(Some(DownloadedUpdate { update, bytes }))
}

async fn install_downloaded_update(
    app: &tauri::AppHandle,
    download: DownloadedUpdate,
) -> Result<()> {
    let DownloadedUpdate { update, bytes } = download;
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
    if state.current().state.is_in_progress() {
        return;
    }
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
            UpdateState::Failed {
                message: "Не удалось проверить обновления. Повторим автоматически.".into(),
                version: None,
            }
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
    use crate::ipc_test;
    use serde_json::{Value, json};
    use tauri::test::{mock_builder, mock_context, noop_assets};

    #[test]
    fn ipc_cancel_stops_the_native_task_and_restores_the_available_version() {
        struct OnDrop(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    sender.send(()).ok();
                }
            }
        }

        let signal = Arc::new(RevisionSignal::new(UpdateState::Downloading {
            version: "9.9.9".into(),
            received: 1024,
            total: Some(4096),
        }));
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (stopped, stopped_rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let _on_drop = OnDrop(Some(stopped));
            started.send(()).expect("task started");
            core::future::pending::<()>().await;
        });
        tauri::async_runtime::block_on(started_rx).expect("download task running");
        let app = mock_builder()
            .manage(Arc::clone(&signal))
            .manage(Mutex::new(UpdateJob {
                generation: 7,
                task: Some(task),
                ready: None,
            }))
            .invoke_handler(tauri::generate_handler![get_update_state, cancel_update])
            .build(mock_context(noop_assets()))
            .expect("test app");
        let window = ipc_test::window(&app);
        assert_eq!(
            ipc_test::invoke(&window, "cancel_update", json!({})),
            Ok(Value::Null)
        );
        assert_eq!(
            ipc_test::invoke(&window, "get_update_state", json!({ "lastRevision": null })),
            Ok(json!({ "revision": 1, "status": "available", "version": "9.9.9" }))
        );
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(1), stopped_rx)
                .await
                .expect("cancel drops the running download")
                .expect("download dropped");
        });
        let manager = app.state::<Mutex<UpdateJob>>();
        let job = manager.lock().expect("job lock");
        assert_eq!(job.generation, 8);
        assert!(job.task.is_none());
        assert!(job.ready.is_none());
    }

    #[test]
    fn ipc_observation_waits_for_a_new_revision_and_installation_cannot_be_cancelled() {
        let signal = Arc::new(RevisionSignal::new(UpdateState::Checking));
        let app = mock_builder()
            .manage(Arc::clone(&signal))
            .manage(Mutex::new(UpdateJob::default()))
            .invoke_handler(tauri::generate_handler![get_update_state, cancel_update])
            .build(mock_context(noop_assets()))
            .expect("test app");
        let window = ipc_test::window(&app);
        let next = Arc::clone(&signal);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            next.notify(UpdateState::Installing {
                version: "9.9.9".into(),
            });
        });
        assert_eq!(
            ipc_test::invoke(&window, "get_update_state", json!({ "lastRevision": 0 })),
            Ok(json!({ "revision": 1, "status": "installing", "version": "9.9.9" }))
        );
        assert_eq!(
            ipc_test::invoke(&window, "cancel_update", json!({})),
            Ok(Value::Null)
        );
        assert!(matches!(
            signal.current().state,
            UpdateState::Installing { .. }
        ));
        assert_eq!(signal.current().revision, 1);
    }

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
