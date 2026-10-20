//! In-memory list of agent tokens in the user's personal Hub hub. Token values are secrets:
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
    tokens: AgentTokens,
    loading: bool,
    error: Option<String>,
    /// Bumped by `clear`, so a refresh started for a previous session is discarded.
    #[serde(skip)]
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
enum TokensError {
    /// The saved profile predates `personal_hub_id`; signing in again stores it.
    NoHub,
    Request(HubError),
}

impl TokensError {
    const fn message(self) -> &'static str {
        match self {
            Self::NoHub => "Войдите в аккаунт CoreInfra Hub заново",
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
        signal.update(|tokens| {
            if tokens.generation != generation {
                return None;
            }
            Some(match result {
                Ok(list) => HubTokens {
                    tokens: list,
                    loading: false,
                    error: None,
                    generation,
                },
                // A rejected session is signed out, which clears the list without an error.
                Err(TokensError::Request(HubError::SignedOut)) => HubTokens {
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

async fn fetch(app: &tauri::AppHandle) -> Result<AgentTokens, TokensError> {
    let hub_id = settings::get_state(app)
        .ok()
        .and_then(|s| s.hub_profile)
        .map(|profile| profile.personal_hub_id)
        .filter(|id| !id.is_empty())
        .ok_or(TokensError::NoHub)?;
    with_session(app, |client, session| async move {
        hub::list_agent_tokens(&client, &session, &hub_id).await
    })
    .await
    .map_err(TokensError::Request)
}

/// Runs an authenticated Hub request with the saved session and applies its `SessionUpdate`:
/// a refreshed cookie is saved and a rejected session signs Wizard out, both only if the saved
/// session is still the one sent.
async fn with_session<T, F, Fut>(app: &tauri::AppHandle, request: F) -> Result<T, HubError>
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
    if settings::update_state(app, move |s| {
        update.apply(s, &session);
    })
    .await
    .is_err()
    {
        log::error!("hub session update failed");
    }
    if signing_out {
        HubTokensManager::clear(app);
        hub_auth::republish(app);
    }
    result
}
