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
use wizard_core::{
    agents::TokenAgent,
    hub::{self, AgentTokens, Client, HubError, SessionUpdate},
    media::fingerprint,
};

pub(crate) const PROXY_TOKEN_NAME: &str = "coreinfra-ai-wizard";

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

fn generation(app: &tauri::AppHandle) -> u64 {
    app.state::<RevisionSignal<HubTokens>>()
        .current()
        .state
        .generation
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
    settings: tauri::State<'_, RevisionSignal<wizard_settings::Settings>>,
) -> Result<RevisionSnapshot<HubTokens>, String> {
    let mut snapshot = signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить токены CoreInfra Hub".to_owned())?;
    let proxy_token = settings.current().state.proxy_token;
    snapshot.state.tokens.retain(|token| {
        token.name != PROXY_TOKEN_NAME && Some(token.token.as_str()) != proxy_token.as_deref()
    });
    Ok(snapshot)
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
    /// With a saved session, starts loading the list.
    pub(crate) fn start_loading(app: &tauri::AppHandle) -> anyhow::Result<()> {
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

// Serializes Wizard token lookup/creation across proxy setup and desktop update operations.
static PROXY_TOKEN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// No Debug/Serialize: this credential never belongs in a prepared frontend plan.
pub(crate) struct ProxyToken {
    pub(crate) value: String,
    pub(crate) session: String,
}

/// Wizard's own credential for local proxy routing and desktop checks/downloads.
/// Always resolves it in the current account's personal Hub, independently of agent tokens.
/// A failed list request must not be mistaken for a missing token and create a duplicate.
pub(crate) async fn proxy_token(app: &tauri::AppHandle) -> Result<ProxyToken, String> {
    let _guard = PROXY_TOKEN_LOCK.lock().await;
    let mut session = settings::get_state(app)
        .map_err(|_| "Настройки недоступны")?
        .hub_session;
    if session.is_empty() {
        return Err("Войдите в CoreInfra Hub".into());
    }
    let client = hub::client().map_err(|_| "Не удалось подключиться к CoreInfra Hub")?;
    // Read the personal Hub in this session chain rather than using a cached profile.
    // Only refreshes returned by our own requests may advance the expected cookie.
    let response = hub::fetch_profile(&client, &session).await;
    let profile = accept_token_response(app, &mut session, response, PROXY_LOOKUP_ERROR).await?;
    let hub_id = &profile.personal_hub_id;
    let response = hub::list_agent_tokens(&client, &session, hub_id).await;
    let tokens = accept_token_response(app, &mut session, response, PROXY_LOOKUP_ERROR).await?;
    let value = resolve_proxy_token(&tokens, async {
        let response = hub::issue_agent_token(&client, &session, hub_id, PROXY_TOKEN_NAME).await;
        let token = accept_token_response(app, &mut session, response, PROXY_LOOKUP_ERROR).await?;
        log::info!("Wizard proxy token issued in personal Hub");
        Ok(token)
    })
    .await?;
    Ok(ProxyToken { value, session })
}

async fn resolve_proxy_token(
    tokens: &AgentTokens,
    issue: impl Future<Output = Result<String, String>>,
) -> Result<String, String> {
    if let Some(token) = tokens.named(PROXY_TOKEN_NAME) {
        Ok(token.token.clone())
    } else {
        issue.await
    }
}

// Preserve refreshes even on response errors, but never adopt an unrelated session
// from settings. Callers saving the token recheck its session under the settings lock.
async fn accept_token_response<T>(
    app: &tauri::AppHandle,
    session: &mut String,
    (update, result): (SessionUpdate, Result<T, HubError>),
    failure: &'static str,
) -> Result<T, String> {
    let next = match &update {
        SessionUpdate::Refreshed(refreshed) => refreshed.clone(),
        SessionUpdate::Unchanged => session.clone(),
        SessionUpdate::Rejected => String::new(),
    };
    save_session_update(app, session, update)
        .await
        .map_err(|_| "Не удалось сохранить сессию CoreInfra Hub")?;
    if !settings::get_state(app).is_ok_and(|s| s.hub_session == next) {
        return Err("Сессия CoreInfra Hub изменилась; повторите применение".into());
    }
    *session = next;
    result.map_err(|error| {
        log::error!("Wizard Hub token request failed: {error:?}");
        failure.into()
    })
}

const PROXY_LOOKUP_ERROR: &str = "Не удалось получить токен Wizard в личном Hub";
const CODEX_LOOKUP_ERROR: &str = "Не удалось получить ключ Codex в выбранном хабе";
static CODEX_TOKEN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Both automatic Codex setup and media use the same default in Wizard's selected Hub.
/// Capture the selection now, even when the caller awaits this after other I/O.
pub(crate) fn codex_token(
    app: &tauri::AppHandle,
) -> impl Future<Output = Result<String, String>> + Send + '_ {
    let revision = generation(app);
    let selected = settings::get_state(app)
        .map_err(|_| "Настройки Wizard недоступны")
        .and_then(|s| {
            if s.hub_session.is_empty() {
                Err("Войдите в CoreInfra Hub в Wizard")
            } else {
                Ok(s.active_hub_id)
            }
        });
    async move {
        let selected = selected?;
        let current = || {
            let state = settings::get_state(app).map_err(|_| "Настройки Wizard недоступны")?;
            if state.hub_session.is_empty()
                || state.active_hub_id != selected
                || generation(app) != revision
            {
                return Err("Хаб или аккаунт изменился во время запроса");
            }
            Ok(state)
        };
        current()?;
        let profile = hub_auth::load_profile(app)
            .await
            .map_err(|_| CODEX_LOOKUP_ERROR)?;
        let hub_id = profile.active_hub_id(&selected).to_owned();
        let client = hub::client().map_err(|_| CODEX_LOOKUP_ERROR)?;
        let list = async {
            let state = current()?;
            let mut session = state.hub_session;
            let response = hub::list_agent_tokens(&client, &session, &hub_id).await;
            let mut tokens =
                accept_token_response(app, &mut session, response, CODEX_LOOKUP_ERROR).await?;
            tokens.retain(|t| Some(t.token.as_str()) != state.proxy_token.as_deref());
            Ok((tokens, session))
        };
        let token = resolve_codex_token(None, list, async |mut session| {
            current()?;
            let response =
                hub::issue_agent_token(&client, &session, &hub_id, TokenAgent::Codex.token_name())
                    .await;
            let issued =
                accept_token_response(app, &mut session, response, CODEX_LOOKUP_ERROR).await?;
            HubTokensManager::refresh(app).await;
            Ok(issued)
        })
        .await?;
        current()?;
        Ok(token)
    }
}

async fn resolve_codex_token(
    original_key_id: Option<&str>,
    list: impl Future<Output = Result<(AgentTokens, String), String>>,
    issue: impl AsyncFnOnce(String) -> Result<String, String>,
) -> Result<String, String> {
    // New jobs and Codex setup share this list-and-issue operation. Polls never issue.
    let _guard = if original_key_id.is_none() {
        Some(CODEX_TOKEN_LOCK.lock().await)
    } else {
        None
    };
    let (tokens, session) = list.await?;
    let name = TokenAgent::Codex.token_name();
    if let Some(id) = original_key_id {
        return tokens
            .iter()
            .find(|t| t.name == name && fingerprint(&t.token) == id)
            .map(|t| t.token.clone())
            .ok_or_else(|| {
                "Исходный ключ Codex для этого видеозадания отозван или недоступен".into()
            });
    }
    if let Some(token) = tokens.named(name) {
        return Ok(token.token.clone());
    }
    issue(session).await
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
    // Preserve existing request semantics: save failures are logged separately.
    let _ = save_session_update(app, &session, update).await;
    result
}

async fn save_session_update(
    app: &tauri::AppHandle,
    session: &str,
    update: SessionUpdate,
) -> Result<(), HubError> {
    if update == SessionUpdate::Unchanged {
        return Ok(());
    }
    let session = session.to_owned();
    let signing_out = update == SessionUpdate::Rejected;
    let cleanup_app = app.clone();
    let saved = settings::update_state(app, move |s| {
        // A later login may have replaced the rejected session; keep that account's state.
        if signing_out && s.hub_session == session {
            hub_auth::clear_profile(&cleanup_app);
            HubTokensManager::clear(&cleanup_app);
        }
        update.apply(s, &session);
    })
    .await
    .map_err(|_| {
        log::error!("hub session update failed");
        HubError::Request
    });
    if signing_out {
        hub_auth::republish(app);
    }
    saved
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
    fn codex_and_video_share_the_oldest_default_and_poll_with_the_original_key() {
        tauri::async_runtime::block_on(async {
            let tokens: AgentTokens = serde_json::from_value(serde_json::json!([
                {"name":"agent-codex", "token":"recent", "issued_at":3},
                {"name":"studio", "token":"custom", "issued_at":1},
                {"name":"coreinfra-ai-wizard", "token":"proxy", "issued_at":1},
                {"name":"agent-codex", "token":"original", "issued_at":2}
            ]))
            .expect("fixture");
            let resolve = |id| {
                resolve_codex_token(id, ready(Ok((tokens.clone(), String::new()))), async |_| {
                    panic!("existing defaults and job lookups must not issue keys")
                })
            };
            assert_eq!(resolve(None).await.as_deref(), Ok("original"));
            assert_eq!(
                resolve(Some(&fingerprint("recent"))).await.as_deref(),
                Ok("recent")
            );
            for missing in ["revoked", "custom", "proxy"] {
                assert!(
                    resolve_codex_token(
                        Some(&fingerprint(missing)),
                        ready(Ok((tokens.clone(), String::new()))),
                        async |_| { panic!("polling must never issue a replacement") }
                    )
                    .await
                    .is_err()
                );
            }
            assert!(
                resolve_codex_token(None, ready(Err("Hub unavailable".into())), async |_| {
                    panic!("failed discovery must not issue a key")
                })
                .await
                .is_err()
            );
        });
    }

    #[test]
    fn simultaneous_first_requests_issue_one_codex_key() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        let issues = AtomicUsize::new(0);
        tauri::async_runtime::block_on(async {
            let request = || {
                resolve_codex_token(
                    None,
                    async {
                        let tokens = if issues.load(Ordering::SeqCst) == 0 {
                            AgentTokens::default()
                        } else {
                            serde_json::from_value(serde_json::json!([
                                {"name":"agent-codex","token":"issued","issued_at":1}
                            ]))
                            .expect("fixture")
                        };
                        tokio::task::yield_now().await;
                        Ok((tokens, "refreshed-session".into()))
                    },
                    async |session| {
                        assert_eq!(session, "refreshed-session");
                        tokio::task::yield_now().await;
                        issues.fetch_add(1, Ordering::SeqCst);
                        Ok("issued".into())
                    },
                )
            };
            let (codex, media) = tokio::join!(request(), request());
            assert_eq!(codex.as_deref(), Ok("issued"));
            assert_eq!(media, codex);
            assert_eq!(issues.load(Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn wizard_operations_reuse_only_the_common_token() {
        let tokens = serde_json::from_value(serde_json::json!([
            { "token": "claude-key", "name": "agent-claude-desktop", "issued_at": 1 },
            { "token": "codex-key", "name": "agent-codex", "issued_at": 2 },
            { "token": "wizard-key", "name": "coreinfra-ai-wizard", "issued_at": 3 }
        ]))
        .expect("test tokens");
        let result = tauri::async_runtime::block_on(resolve_proxy_token(&tokens, async {
            panic!("an existing Wizard token must not be duplicated")
        }));
        assert_eq!(result.as_deref(), Ok("wizard-key"));
    }

    #[test]
    fn missing_wizard_token_never_falls_back_to_agent_credentials() {
        let tokens = serde_json::from_value(serde_json::json!([
            { "token": "claude-key", "name": "agent-claude-desktop", "issued_at": 1 },
            { "token": "codex-key", "name": "agent-codex", "issued_at": 2 }
        ]))
        .expect("test tokens");
        let issued = tauri::async_runtime::block_on(resolve_proxy_token(
            &tokens,
            ready(Ok("new-wizard-key".into())),
        ));
        assert_eq!(issued.as_deref(), Ok("new-wizard-key"));
        let failed = tauri::async_runtime::block_on(resolve_proxy_token(
            &tokens,
            ready(Err("Hub unavailable".into())),
        ));
        assert_eq!(failed, Err("Hub unavailable".into()));
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
