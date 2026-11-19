use crate::{hub_auth, hub_tokens, settings};
use anyhow::{Context as _, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use wizard_core::{
    hub::{self, AgentToken, AgentTokens},
    video::{self, VideoHub, server::VideoAccess},
};
use wizard_settings::VideoConnection;

#[derive(Default)]
pub(crate) struct Runtime {
    listener: Option<JoinHandle<()>>,
    error: Option<String>,
}

pub(crate) type State = Mutex<Runtime>;

pub(crate) fn selectable(token: &AgentToken, proxy: Option<&str>) -> bool {
    !token.name.trim().is_empty()
        && token.name != hub_tokens::PROXY_TOKEN_NAME
        && Some(token.token.as_str()) != proxy
}

#[derive(Serialize)]
pub(crate) struct VideoKey {
    name: String,
    id: String,
}

impl From<&AgentToken> for VideoKey {
    fn from(t: &AgentToken) -> Self {
        Self {
            name: t.name.clone(),
            id: video::fingerprint(&t.token),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Selection {
    hub_id: String,
    name: String,
    id: Option<String>,
}

fn choose<'a>(
    list: &'a AgentTokens,
    selection: &Selection,
    proxy: Option<&str>,
) -> Result<&'a AgentToken> {
    let mut matches = list.iter().filter(|t| {
        selectable(t, proxy)
            && t.name == selection.name
            && selection
                .id
                .as_ref()
                .is_none_or(|id| *id == video::fingerprint(&t.token))
    });
    let token = matches
        .next()
        .context("Выбранный ключ отсутствует в хабе")?;
    ensure!(
        matches.next().is_none(),
        "В хабе несколько ключей с таким именем. Выберите конкретный ключ"
    );
    Ok(token)
}

async fn list(app: &tauri::AppHandle, hub_id: String) -> Result<AgentTokens> {
    hub_tokens::with_session(app, |client, session| async move {
        hub::list_agent_tokens(&client, &session, &hub_id).await
    })
    .await
    .map_err(|_| anyhow!("Не удалось получить ключи выбранного хаба"))
}

struct Access(tauri::AppHandle);

impl VideoAccess for Access {
    fn connection(&self) -> Option<VideoConnection> {
        let state = settings::get_state(&self.0).ok()?;
        if state.hub_session.is_empty() {
            None
        } else {
            state.video
        }
    }

    async fn token(&self, connection: &VideoConnection) -> Result<String> {
        let generation = hub_tokens::generation(&self.0);
        let profile = hub_auth::load_profile(&self.0)
            .await
            .map_err(|_| anyhow!("Войдите в CoreInfra Hub в Wizard"))?;
        ensure!(
            profile.personal_hub_id == connection.account_id,
            "Войдите в исходный аккаунт CoreInfra Hub"
        );
        let tokens = list(&self.0, connection.hub_id.clone()).await?;
        let state = settings::get_state(&self.0)?;
        ensure!(
            !state.hub_session.is_empty()
                && state.video.as_ref() == Some(connection)
                && generation == hub_tokens::generation(&self.0),
            "Подключение видео изменилось"
        );
        tokens
            .iter()
            .find(|t| {
                selectable(t, state.proxy_token.as_deref())
                    && video::fingerprint(&t.token) == connection.key_id
            })
            .map(|t| t.token.clone())
            .context("Исходный ключ видео отозван или недоступен")
    }
}

impl Runtime {
    async fn start(&mut self, app: &tauri::AppHandle) -> Result<()> {
        if self.listener.as_ref().is_some_and(|t| !t.is_finished()) {
            return Ok(());
        }
        let hub = VideoHub::new()?;
        let listener = TcpListener::bind(video::ADDRESS)
            .await
            .context("Не удалось открыть локальный порт видео 33789")?;
        let access = Access(app.clone());
        self.listener = Some(tokio::spawn(async move {
            if let Err(e) = video::server::serve(listener, hub, access).await {
                log::warn!("video MCP listener stopped: {e}");
            }
        }));
        Ok(())
    }

    async fn stop(&mut self) {
        if let Some(task) = self.listener.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

pub(crate) async fn initialize(app: &tauri::AppHandle) {
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let result = async {
        let config = settings::get_state(app)?;
        wizard_core::agents::codex::migrate_hub_key_filter()?;
        if let Some(current) = config.video {
            runtime.start(app).await?;
            video::install::sync(Some(&current), Some(&current))?;
        }
        anyhow::Ok(())
    }
    .await;
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
    hub_id: Option<String>,
    key_id: Option<String>,
    key_name: Option<String>,
    keys_hub_id: Option<String>,
    keys: Vec<VideoKey>,
    running: bool,
    error: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_video_connection(app: tauri::AppHandle) -> Result<Status, String> {
    let state = app.state::<State>();
    let runtime = state.lock().await;
    let current = settings::get_state(&app)
        .map_err(|_| "Настройки недоступны")?
        .video;
    let (keys_hub_id, keys) = hub_tokens::video_keys(&app);
    Ok(Status {
        hub_id: current.as_ref().map(|c| c.hub_id.clone()),
        key_id: current.as_ref().map(|c| c.key_id.clone()),
        key_name: current.map(|c| c.key_name),
        keys_hub_id,
        keys,
        running: runtime.listener.as_ref().is_some_and(|t| !t.is_finished()),
        error: runtime.error.clone(),
    })
}

async fn configure(
    app: &tauri::AppHandle,
    runtime: &mut Runtime,
    selection: Option<Selection>,
) -> Result<()> {
    let current = settings::get_state(app)?.video;
    let Some(selection) = selection else {
        settings::update_state(app, |s| s.video = None).await?;
        runtime.stop().await;
        return video::install::sync(current.as_ref(), None);
    };
    let generation = hub_tokens::generation(app);
    let profile = hub_auth::load_profile(app)
        .await
        .map_err(|_| anyhow!("Войдите в CoreInfra Hub"))?;
    ensure!(
        selection.hub_id == profile.active_hub_id(&settings::get_state(app)?.active_hub_id),
        "Выбранный хаб изменился"
    );
    let tokens = list(app, selection.hub_id.clone()).await?;
    let snapshot = settings::get_state(app)?;
    ensure!(
        selection.hub_id == profile.active_hub_id(&snapshot.active_hub_id),
        "Выбранный хаб изменился"
    );
    let token = choose(&tokens, &selection, snapshot.proxy_token.as_deref())?;
    let key_id = video::fingerprint(&token.token);
    let next = VideoConnection {
        local_bearer: current
            .as_ref()
            .filter(|c| {
                c.account_id == profile.personal_hub_id
                    && c.hub_id == selection.hub_id
                    && c.key_id == key_id
            })
            .map_or_else(
                || format!("wizard-video-{}", uuid::Uuid::new_v4()),
                |c| c.local_bearer.clone(),
            ),
        account_id: profile.personal_hub_id,
        hub_id: selection.hub_id,
        key_name: token.name.clone(),
        key_id,
    };
    runtime.start(app).await?;
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let guard = settings::lock_state()?;
        let now = settings::get_state(&handle)?;
        ensure!(
            !now.hub_session.is_empty()
                && now.hub_session == snapshot.hub_session
                && now.active_hub_id == snapshot.active_hub_id
                && generation == hub_tokens::generation(&handle),
            "Хаб или аккаунт изменился. Повторите подключение"
        );
        video::install::sync(current.as_ref(), Some(&next))?;
        if let Err(e) =
            settings::update_state_locked(&handle, &guard, |s| s.video = Some(next.clone()))
        {
            video::install::sync(Some(&next), current.as_ref())
                .context("Не удалось восстановить регистрацию видео")?;
            return Err(e);
        }
        Ok(())
    })
    .await
    .context("Video configuration task failed")?;
    if result.is_err() && settings::get_state(app)?.video.is_none() {
        runtime.stop().await;
    }
    result
}

#[tauri::command]
pub(crate) async fn set_video_connection(
    app: tauri::AppHandle,
    selection: Option<Selection>,
) -> Result<(), String> {
    let state = app.state::<State>();
    let mut runtime = state.lock().await;
    let result = configure(&app, &mut runtime, selection)
        .await
        .map_err(|e| format!("{e:#}"));
    runtime.error = result.as_ref().err().cloned();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_hub_scoped_and_duplicates_require_the_selected_fingerprint() -> Result<()> {
        let tokens: AgentTokens = serde_json::from_value(serde_json::json!([
            {"name":"studio", "token":"a", "issued_at":1}, {"name":"studio", "token":"b", "issued_at":1},
            {"name":"work", "token":"c", "issued_at":1}, {"name":"proxy-alias", "token":"proxy", "issued_at":1},
            {"name":"coreinfra-ai-wizard", "token":"reserved", "issued_at":1}
        ]))?;
        let mut choice = Selection {
            hub_id: "h".into(),
            name: "studio".into(),
            id: None,
        };
        assert!(choose(&tokens, &choice, None).is_err());
        choice.id = Some(video::fingerprint("b"));
        assert_eq!(choose(&tokens, &choice, None)?.token, "b");
        choice.name = "work".into();
        assert!(choose(&tokens, &choice, None).is_err());
        choice.id = None;
        assert_eq!(choose(&tokens, &choice, None)?.token, "c");
        for name in ["missing", "coreinfra-ai-wizard", "proxy-alias"] {
            choice.name = name.into();
            assert!(choose(&tokens, &choice, Some("proxy")).is_err());
        }
        Ok(())
    }
}
