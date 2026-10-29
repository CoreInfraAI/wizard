//! In-memory list of agent tokens in the selected Hub. Token values are secrets:
//! they are never persisted or logged and leave the backend only through IPC.

use crate::{
    hub_auth,
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use core::future::Future;
use serde::Serialize;
use tauri::Manager as _;
use wizard_core::hub::{self, AgentTokens, Client, HubError, SessionUpdate};

#[derive(Clone, Default, Serialize)]
pub(crate) struct HubTokens {
    hub_id: Option<String>,
    tokens: AgentTokens,
    loading: bool,
    error: Option<String>,
    /// Bumped by `clear`, so a refresh or issue started for a previous session is discarded.
    #[serde(skip)]
    generation: u64,
}

impl HubTokens {
    /// Use only a successfully loaded list; never issue a token for a background operation.
    pub(crate) fn named_token(&self, name: &str) -> Option<String> {
        if self.loading || self.error.is_some() {
            return None;
        }
        self.tokens.named(name).map(|token| token.token.clone())
    }
}

#[derive(Clone, Copy, Debug)]
enum TokensError {
    /// Hub returned a profile without `personal_hub_id`.
    NoHub,
    Request(HubError),
}

impl TokensError {
    const fn message(self) -> &'static str {
        match self {
            Self::NoHub => "CoreInfra Hub не вернул hub аккаунта",
            Self::Request(_) => "Не удалось загрузить токены CoreInfra Hub",
        }
    }
}

/// Returns the token list once it differs from `last_revision`.
#[tauri::command]
pub(crate) async fn get_hub_tokens_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<HubTokens>>,
) -> Result<RevisionSnapshot<HubTokens>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить токены CoreInfra Hub".into())
}

/// Reloads the token list; failures are published in its state.
#[tauri::command]
pub(crate) async fn refresh_hub_tokens(app: tauri::AppHandle) {
    HubTokensManager::refresh(&app).await;
}

#[derive(Clone, Serialize)]
pub(crate) struct ConfiguredTokenInfo {
    name: String,
    hub_id: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct ConfiguredTokens {
    details: Vec<Option<ConfiguredTokenInfo>>,
    error: Option<String>,
}

/// Resolve already configured keys across the account's hubs, independently of the selected
/// hub. Refresh with the token list; network errors are state, IPC errors reject the command.
/// Return only names and hub IDs, in request order; never log the input keys.
#[tauri::command]
pub(crate) async fn get_configured_token_info(
    app: tauri::AppHandle,
    tokens: Vec<String>,
    last_revision: Option<u32>,
) -> Result<RevisionSnapshot<ConfiguredTokens>, String> {
    let signal = app.state::<RevisionSignal<HubTokens>>();
    let mut snapshot = signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить сведения о ключах")?;
    // Wait for a completed refresh rather than querying every hub twice per refresh.
    while snapshot.state.loading {
        snapshot = signal
            .wait(Some(snapshot.revision))
            .await
            .map_err(|_| "Не удалось получить сведения о ключах")?;
    }
    let state = match hub_auth::load_profile(&app).await {
        Ok(profile) => {
            resolve_configured_tokens(
                &tokens,
                profile.hubs.into_iter().map(|hub| hub.hub_id),
                |hub_id| {
                    with_session(&app, |client, session| async move {
                        hub::list_agent_tokens(&client, &session, &hub_id).await
                    })
                },
            )
            .await
        }
        Err(_) => ConfiguredTokens {
            details: vec![None; tokens.len()],
            error: Some("Не удалось загрузить сведения о хабах настроенных агентов".into()),
        },
    };
    Ok(RevisionSnapshot {
        revision: snapshot.revision,
        state,
    })
}

async fn resolve_configured_tokens<F, Fut>(
    tokens: &[String],
    hubs: impl IntoIterator<Item = String>,
    mut fetch: F,
) -> ConfiguredTokens
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<AgentTokens, HubError>>,
{
    let mut result = ConfiguredTokens {
        details: vec![None; tokens.len()],
        error: None,
    };
    for hub_id in hubs {
        if result.details.iter().all(Option::is_some) {
            break;
        }
        let list = match fetch(hub_id.clone()).await {
            Ok(list) => list,
            Err(HubError::SignedOut) => {
                result.error = Some("Войдите в CoreInfra Hub".into());
                break;
            }
            Err(_) => {
                result.error =
                    Some("Не удалось загрузить сведения о ключах из CoreInfra Hub".into());
                continue;
            }
        };
        for (token, info) in tokens.iter().zip(&mut result.details) {
            if let Some(found) = list.get(token) {
                *info = Some(ConfiguredTokenInfo {
                    name: found.name.clone(),
                    hub_id: hub_id.clone(),
                });
            }
        }
    }
    // A failure in an unrelated hub does not invalidate keys resolved in other hubs.
    if result.details.iter().all(Option::is_some) {
        result.error = None;
    }
    result
}

/// Loads, publishes and clears the token list, and runs authenticated Hub requests.
pub(crate) struct HubTokensManager;

impl HubTokensManager {
    /// Registers the empty list and, with a saved session, starts loading it.
    pub(crate) fn initialize(app: &tauri::AppHandle) -> anyhow::Result<()> {
        app.manage(RevisionSignal::new(HubTokens::default()));
        if !settings::get_state(app)?.hub_session.is_empty() {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { Self::refresh(&app).await });
        }
        Ok(())
    }

    /// Reloads the list from Hub, keeping the previous tokens until the new ones arrive.
    pub(crate) async fn refresh(app: &tauri::AppHandle) {
        let signal = app.state::<RevisionSignal<HubTokens>>();
        let generation = signal.current().state.generation;
        signal.update(|tokens| {
            Some(HubTokens {
                loading: true,
                error: None,
                ..tokens.clone()
            })
        });

        let result = fetch(app).await;
        if let Err(error) = result {
            log::warn!("hub tokens load failed: {error:?}");
        }
        // A 401 for a session that was replaced meanwhile is an error, not a sign-out.
        let signed_out = settings::get_state(app).is_ok_and(|s| s.hub_session.is_empty());
        signal.update(|tokens| {
            if tokens.generation != generation {
                return None;
            }
            Some(match result {
                Ok((hub_id, list)) => HubTokens {
                    hub_id: Some(hub_id),
                    tokens: list,
                    loading: false,
                    error: None,
                    generation,
                },
                // A rejected session is signed out, which clears the list without an error.
                Err(TokensError::Request(HubError::SignedOut)) if signed_out => HubTokens {
                    generation,
                    ..HubTokens::default()
                },
                Err(error) => HubTokens {
                    loading: false,
                    error: Some(error.message().to_owned()),
                    ..tokens.clone()
                },
            })
        });
    }

    /// Forgets the list on logout or a rejected session and discards in-flight refreshes.
    pub(crate) fn clear(app: &tauri::AppHandle) {
        app.state::<RevisionSignal<HubTokens>>().update(|tokens| {
            Some(HubTokens {
                generation: tokens.generation.wrapping_add(1),
                ..HubTokens::default()
            })
        });
    }
}

/// Creates a token named `name` in the selected hub, then reloads the list so the token shows
/// with Hub's own data before its value is returned. A failed reload is published as the list
/// error and does not fail the result. Never log the value.
pub(crate) async fn issue(app: &tauri::AppHandle, name: &str) -> Result<String, String> {
    let signal = app.state::<RevisionSignal<HubTokens>>();
    let generation = signal.current().state.generation;
    let hub_id = active_hub_id(app).await.map_err(|error| match error {
        TokensError::NoHub => error.message().to_owned(),
        TokensError::Request(_) => "Не удалось создать токен CoreInfra Hub".to_owned(),
    })?;
    let request_name = name.to_owned();
    let result = with_session(app, |client, session| async move {
        hub::issue_agent_token(&client, &session, &hub_id, &request_name).await
    })
    .await;
    if let Err(error) = &result {
        log::warn!("hub token '{name}' issue failed: {error:?}");
    }
    let token = result.map_err(|_| "Не удалось создать токен CoreInfra Hub".to_owned())?;
    log::info!("hub token '{name}' issued");
    HubTokensManager::refresh(app).await;

    // A cleared list belongs to a signed-out session: the token is not used there. Check after
    // the refresh, which a logout can also interrupt.
    if signal.current().state.generation != generation {
        log::warn!("hub token '{name}' issued for a cleared session");
        return Err("Сессия CoreInfra Hub изменилась во время создания токена".into());
    }
    Ok(token)
}

async fn active_hub_id(app: &tauri::AppHandle) -> Result<String, TokensError> {
    let profile = hub_auth::load_profile(app)
        .await
        .map_err(TokensError::Request)?;
    let selected = settings::get_state(app)
        .map_err(|_| TokensError::Request(HubError::Request))?
        .active_hub_id;
    Some(profile.active_hub_id(&selected).to_owned())
        .filter(|id| !id.is_empty())
        .ok_or(TokensError::NoHub)
}

async fn fetch(app: &tauri::AppHandle) -> Result<(String, AgentTokens), TokensError> {
    let hub_id = active_hub_id(app).await?;
    with_session(app, |client, session| async move {
        let (update, tokens) = hub::list_agent_tokens(&client, &session, &hub_id).await;
        (update, tokens.map(|tokens| (hub_id, tokens)))
    })
    .await
    .map_err(TokensError::Request)
}

/// Runs an authenticated Hub request with the saved session and applies its `SessionUpdate`:
/// a refreshed cookie is saved and a rejected session signs Wizard out, both only if the saved
/// session is still the one sent.
pub(crate) async fn with_session<T, F, Fut>(
    app: &tauri::AppHandle,
    request: F,
) -> Result<T, HubError>
where
    F: FnOnce(Client, String) -> Fut,
    Fut: Future<Output = (SessionUpdate, Result<T, HubError>)>,
{
    let session = settings::get_state(app)
        .map_err(|_| HubError::Request)?
        .hub_session;
    let client = hub::client()?;
    let (update, result) = request(client, session.clone()).await;
    if update == SessionUpdate::Unchanged {
        return result;
    }

    let signing_out = update == SessionUpdate::Rejected;
    let cleanup_app = app.clone();
    if settings::update_state(app, move |s| {
        // A later login may have replaced the rejected session; keep that account's state.
        if signing_out && s.hub_session == session {
            hub_auth::clear_profile(&cleanup_app);
            HubTokensManager::clear(&cleanup_app);
        }
        update.apply(s, &session);
    })
    .await
    .is_err()
    {
        log::error!("hub session update failed");
    }
    if signing_out {
        hub_auth::republish(app);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::ready;

    fn token_list(token: &str) -> AgentTokens {
        serde_json::from_value(serde_json::json!([
            { "token": token, "name": "Agent", "issued_at": 1 }
        ]))
        .expect("test token list should deserialize")
    }

    #[test]
    fn missing_key_is_not_a_network_error() {
        let result = tauri::async_runtime::block_on(resolve_configured_tokens(
            &["missing-key".into()],
            ["personal".into()],
            |_| ready(Ok(AgentTokens::default())),
        ));
        assert!(result.details[0].is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn partial_failure_keeps_known_keys_and_refresh_recovers() {
        let keys = ["personal-key".into(), "team-key".into()];
        let failed = tauri::async_runtime::block_on(resolve_configured_tokens(
            &keys,
            ["personal".into(), "team".into()],
            |id| {
                ready(if id == "personal" {
                    Ok(token_list("personal-key"))
                } else {
                    Err(HubError::Request)
                })
            },
        ));
        assert_eq!(
            failed.details[0].as_ref().map(|info| info.hub_id.as_str()),
            Some("personal")
        );
        assert!(failed.details[1].is_none());
        assert!(failed.error.is_some());

        let refreshed = tauri::async_runtime::block_on(resolve_configured_tokens(
            &keys,
            ["personal".into(), "team".into()],
            |id| {
                ready(Ok(token_list(if id == "personal" {
                    "personal-key"
                } else {
                    "team-key"
                })))
            },
        ));
        assert!(refreshed.error.is_none());
        assert_eq!(
            refreshed.details[1]
                .as_ref()
                .map(|info| info.hub_id.as_str()),
            Some("team")
        );
    }

    #[test]
    fn unrelated_hub_failure_does_not_hide_resolved_key() {
        let result = tauri::async_runtime::block_on(resolve_configured_tokens(
            &["team-key".into()],
            ["unavailable".into(), "team".into()],
            |id| {
                ready(if id == "team" {
                    Ok(token_list("team-key"))
                } else {
                    Err(HubError::Request)
                })
            },
        ));
        assert!(result.error.is_none());
        assert_eq!(
            result.details[0].as_ref().map(|info| info.name.as_str()),
            Some("Agent")
        );
    }
}
