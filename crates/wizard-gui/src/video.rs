use crate::{hub_tokens, settings};
use anyhow::{Context as _, Result};
use core::future::Future;
use serde::Serialize;
use tauri::Manager as _;
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use wizard_core::video::{self, Job, VideoHub, VideoKey, server::VideoAccess};

#[derive(Default)]
pub(crate) struct Runtime {
    listener: Option<JoinHandle<()>>,
    error: Option<String>,
}

pub(crate) type State = Mutex<Runtime>;

struct Access(tauri::AppHandle);

impl VideoAccess for Access {
    fn token<'a>(
        &'a self,
        job: Option<&'a Job>,
    ) -> impl Future<Output = Result<VideoKey>> + Send + 'a {
        let key = hub_tokens::codex_token(&self.0, job);
        async move { key.await.map_err(anyhow::Error::msg) }
    }
}

impl Runtime {
    fn running(&self) -> bool {
        self.listener
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    async fn stop(&mut self) {
        if let Some(task) = self.listener.take() {
            task.abort();
            let _ = task.await;
        }
    }

    async fn configure(&mut self, app: &tauri::AppHandle, enabled: bool) -> Result<()> {
        if !enabled {
            let previous = settings::get_state(app)?.video.local_bearer;
            settings::update_state(app, |s| {
                s.video.enabled = false;
                s.video.local_bearer = None;
            })
            .await?;
            self.stop().await;
            return tauri::async_runtime::spawn_blocking(move || {
                video::install::sync(previous.as_deref(), None)
            })
            .await
            .context("Video cleanup task failed")?;
        }
        if self.running() {
            return Ok(());
        }
        let hub = VideoHub::new()?;
        let listener = TcpListener::bind(video::ADDRESS)
            .await
            .context("Не удалось открыть локальный порт видео 33789")?;
        let handle = app.clone();
        let access = Access(app.clone());
        self.listener = Some(
            video::server::start(listener, hub, access, move |bearer| {
                let guard = settings::lock_state()?;
                let previous = settings::get_state(&handle)?.video.local_bearer;
                video::install::sync(previous.as_deref(), Some(bearer))?;
                if let Err(error) = settings::update_state_locked(&handle, &guard, |s| {
                    s.video.enabled = true;
                    s.video.local_bearer = Some(bearer.to_owned());
                }) {
                    video::install::sync(Some(bearer), previous.as_deref())
                        .context("Не удалось восстановить регистрацию видео")?;
                    return Err(error);
                }
                Ok(())
            })
            .await?,
        );
        Ok(())
    }
}

pub(crate) async fn initialize(app: &tauri::AppHandle) {
    let enabled = settings::get_state(app).map(|s| s.video.enabled);
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let result = match enabled {
        Ok(enabled) => runtime.configure(app, enabled).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log::warn!("video setup: {error:#}");
        runtime.error = Some(format!("{error:#}"));
    }
}

pub(crate) async fn shutdown(app: &tauri::AppHandle) {
    app.state::<State>().lock().await.stop().await;
}

#[derive(Serialize)]
pub(crate) struct Status {
    enabled: bool,
    running: bool,
    error: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_video_connection(app: tauri::AppHandle) -> Result<Status, String> {
    let state = app.state::<State>();
    let runtime = state.lock().await;
    Ok(Status {
        enabled: settings::get_state(&app)
            .map_err(|_| "Настройки недоступны")?
            .video
            .enabled,
        running: runtime.running(),
        error: runtime.error.clone(),
    })
}

#[tauri::command]
pub(crate) async fn set_video_connection(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let result = runtime
        .configure(&app, enabled)
        .await
        .map_err(|e| format!("{e:#}"));
    runtime.error = result.as_ref().err().cloned();
    result
}
