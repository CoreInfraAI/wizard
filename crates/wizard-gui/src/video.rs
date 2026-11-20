use crate::settings;
use anyhow::{Context as _, Result};
use serde::Serialize;
use tauri::Manager as _;
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use wizard_core::video::{self, VideoHub, VideoKey, keys::VideoKeys};
use wizard_settings::{VideoAgent, VideoSettings};

#[derive(Default)]
pub(crate) struct Runtime {
    listener: Option<JoinHandle<()>>,
    error: Option<String>,
}

pub(crate) type State = Mutex<Runtime>;

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

    async fn reload(&mut self, grants: VideoSettings) -> Result<()> {
        self.stop().await;
        if grants == VideoSettings::default() {
            return Ok(());
        }
        let keys = tauri::async_runtime::spawn_blocking(VideoKeys::load).await?;
        let routes = [
            (grants.codex, keys.codex),
            (grants.claude, keys.claude),
            (grants.claude_desktop, keys.claude_desktop),
        ]
        .into_iter()
        .filter_map(|(bearer, key)| Some((bearer?, key.map(|token| VideoKey { token }))))
        .collect();
        let listener = TcpListener::bind(video::ADDRESS)
            .await
            .context("Не удалось открыть локальный порт видео 33789")?;
        let addr = listener.local_addr()?;
        self.listener = Some(tokio::spawn(video::server::serve(
            listener,
            addr,
            VideoHub::new()?,
            routes,
        )));
        Ok(())
    }
}

pub(crate) async fn initialize(app: &tauri::AppHandle) {
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let result = match settings::get_state(app) {
        Ok(s) => runtime.reload(s.video).await,
        Err(error) => {
            runtime.stop().await;
            Err(error)
        }
    };
    runtime.error = result.err().map(|e| format!("{e:#}"));
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
pub(crate) async fn get_video_connection(
    app: tauri::AppHandle,
    agent: VideoAgent,
) -> Result<Status, String> {
    let state = app.state::<State>();
    let runtime = state.lock().await;
    let enabled = settings::get_state(&app)
        .map_err(|_| "Настройки недоступны")?
        .video
        .bearer(agent)
        .is_some();
    Ok(Status {
        enabled,
        running: enabled && runtime.running(),
        error: if enabled { runtime.error.clone() } else { None },
    })
}

#[tauri::command]
pub(crate) async fn set_video_connection(
    app: tauri::AppHandle,
    agent: VideoAgent,
    enabled: bool,
) -> Result<(), String> {
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let handle = app.clone();
    let grants = tauri::async_runtime::spawn_blocking(move || -> Result<VideoSettings> {
        let guard = settings::lock_state()?;
        let mut grants = settings::get_state(&handle)?.video;
        let previous = grants.bearer(agent);
        let next = enabled.then(|| previous.map_or_else(video::server::new_bearer, str::to_owned));
        video::install::sync(agent, previous, next.as_deref())?;
        if let Err(error) =
            settings::update_state_locked(&handle, &guard, |s| s.video.set(agent, next.clone()))
        {
            video::install::sync(agent, next.as_deref(), previous)
                .context("Не удалось восстановить регистрацию видео")?;
            return Err(error);
        }
        grants.set(agent, next);
        Ok(grants)
    })
    .await
    .map_err(|_| "Не удалось настроить видео")?
    .map_err(|e| format!("{e:#}"))?;
    let result = runtime.reload(grants).await.map_err(|e| format!("{e:#}"));
    runtime.error = result.as_ref().err().cloned();
    result
}
