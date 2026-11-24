use crate::{agents::AgentRevisionState, revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::Serialize;
use tauri::Manager as _;
use tokio::sync::Mutex;
use wizard_core::media;
use wizard_settings::MediaAgent;

pub(crate) type State = Mutex<()>;

#[derive(Serialize)]
pub(crate) struct Status {
    enabled: bool,
    /// Offered by default: no grant yet, and the user has not turned media off.
    suggested: bool,
    running: bool,
    error: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_media_connection(
    app: tauri::AppHandle,
    agent: MediaAgent,
) -> Result<Status, String> {
    let media = settings::get_state(&app)
        .map_err(|_| "Настройки недоступны")?
        .media;
    let enabled = media.bearer(agent).is_some();
    let (running, error) = if enabled {
        match wizard_proxy::control::status().await {
            Ok(status) => (status.media_running, status.media_error),
            Err(_) => (
                false,
                Some("Не удалось получить статус медиасервиса от службы Wizard".into()),
            ),
        }
    } else {
        (false, None)
    };
    Ok(Status {
        enabled,
        suggested: media.suggested(agent),
        running,
        error,
    })
}

#[tauri::command]
pub(crate) async fn set_media_connection(
    app: tauri::AppHandle,
    agent: MediaAgent,
    enabled: bool,
) -> Result<(), String> {
    let state = app.state::<State>();
    let _guard = state.lock().await;
    let handle = app.clone();
    let result: Result<()> = if enabled {
        let saved = tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            let guard = settings::lock_state()?;
            let grants = settings::get_state(&handle)?.media;
            let previous = grants.bearer(agent);
            let next = previous.map_or_else(media::server::new_bearer, str::to_owned);
            media::install::sync(agent, previous, Some(&next))?;
            if let Err(error) = settings::update_state_locked(&handle, &guard, |s| {
                s.media.set(agent, Some(next.clone()));
            }) {
                media::install::sync(agent, Some(&next), previous)
                    .context("Не удалось восстановить регистрацию медиамоделей")?;
                return Err(error);
            }
            Ok(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("Не удалось настроить медиамодели"))
        .flatten();
        // Detection compares registrations with the saved grant.
        app.state::<RevisionSignal<AgentRevisionState>>()
            .notify(AgentRevisionState);
        match saved {
            Ok(()) => crate::proxy::sync(&app).await,
            Err(error) => Err(error),
        }
    } else {
        // Persist revocation and reload the daemon BEFORE cleaning up agent files so a
        // corrupt skill manifest or config error can never leave a revoked bearer active.
        let previous = tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>> {
            let guard = settings::lock_state()?;
            let previous = settings::get_state(&handle)?
                .media
                .bearer(agent)
                .map(str::to_owned);
            settings::update_state_locked(&handle, &guard, |s| s.media.set(agent, None))?;
            Ok(previous)
        })
        .await
        .map_err(|_| anyhow::anyhow!("Не удалось отключить медиамодели"))
        .flatten();
        let previous = match previous {
            Ok(previous) => previous,
            Err(error) => return Err(format!("{error:#}")),
        };
        let daemon_sync = crate::proxy::sync(&app).await;
        let cleanup = tauri::async_runtime::spawn_blocking(move || {
            media::install::sync(agent, previous.as_deref(), None)
        })
        .await
        .map_err(|_| anyhow::anyhow!("Не удалось очистить файлы медиамоделей"))
        .flatten();
        app.state::<RevisionSignal<AgentRevisionState>>()
            .notify(AgentRevisionState);
        daemon_sync.and(cleanup)
    };
    result.map_err(|e| format!("{e:#}"))
}
