use crate::{agents::AgentRevisionState, revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::Serialize;
use tauri::Manager as _;
use tokio::sync::Mutex;
use wizard_core::video;
use wizard_settings::VideoAgent;

pub(crate) type State = Mutex<()>;

#[derive(Serialize)]
pub(crate) struct Status {
    enabled: bool,
    /// Offered by default: no grant yet, and the user has not turned video off.
    suggested: bool,
    running: bool,
    error: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_video_connection(
    app: tauri::AppHandle,
    agent: VideoAgent,
) -> Result<Status, String> {
    let video = settings::get_state(&app)
        .map_err(|_| "Настройки недоступны")?
        .video;
    let enabled = video.bearer(agent).is_some();
    let (running, error) = if enabled {
        match wizard_proxy::control::status().await {
            Ok(status) => (status.video_running, status.video_error),
            Err(_) => (
                false,
                Some("Не удалось получить статус видео от службы Wizard".into()),
            ),
        }
    } else {
        (false, None)
    };
    Ok(Status {
        enabled,
        suggested: video.suggested(agent),
        running,
        error,
    })
}

#[tauri::command]
pub(crate) async fn set_video_connection(
    app: tauri::AppHandle,
    agent: VideoAgent,
    enabled: bool,
) -> Result<(), String> {
    let state = app.state::<State>();
    let _guard = state.lock().await;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<()> {
        let guard = settings::lock_state()?;
        let grants = settings::get_state(&handle)?.video;
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
        Ok(())
    })
    .await
    .map_err(|_| "Не удалось настроить видео")?
    .map_err(|e| format!("{e:#}"))?;
    // Detection compares registrations with the saved grant. A preceding routing plan
    // may already have triggered a read against the previous grant.
    app.state::<RevisionSignal<AgentRevisionState>>()
        .notify(AgentRevisionState);
    crate::proxy::sync(&app).await.map_err(|e| format!("{e:#}"))
}
