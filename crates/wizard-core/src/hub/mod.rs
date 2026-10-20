//! Hub protocol shared by the GUI and CLI: HTTP client, authenticated requests with
//! session refresh, and agent tokens. Nothing here touches saved settings; callers apply
//! `SessionUpdate` themselves. Never include request URLs, headers, cookies, token values or
//! HTTP-client errors in diagnostics.

pub mod login;

use crate::settings::Settings;
use anyhow::anyhow;
use cookie::Cookie;
use core::time::Duration;
pub use reqwest::{Client, Url};
use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, COOKIE, HeaderMap, SET_COOKIE},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

// TEMP: local `cargo x ff-hub --https`; restore https://hub.coreinfra.ai before committing.
const HUB_ORIGIN: &str = "https://hub.local.coreinfra.ai:8443";
const PROFILE_PATH: &str = "/hub/api/profile";
const HUB_INFO_PATH: &str = "/hub/api/hub/info";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Installs ring as the process-wide rustls provider; reqwest is built without one. Call once
/// at startup, before creating HTTP clients.
pub fn install_crypto_provider() -> anyhow::Result<()> {
    tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("rustls crypto provider already installed"))
}

/// HTTP client for Hub: no redirects, no cookie store, `REQUEST_TIMEOUT`.
pub fn client() -> Result<Client, HubError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| HubError::Request)
}

/// Failure of an authenticated Hub request. `SignedOut`: no session, or Hub rejected it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubError {
    SignedOut,
    Request,
    Status(StatusCode),
    Response,
}

/// What an authenticated request means for the saved session.
#[derive(Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    Unchanged,
    /// Hub reissued the session (under 30 days of its 180-day life left).
    Refreshed(String),
    /// Hub answered 401: the session no longer works.
    Rejected,
}

impl SessionUpdate {
    /// Applies the update unless a login or logout already replaced the session `sent`.
    pub fn apply(self, settings: &mut Settings, sent: &str) {
        if settings.hub_session != sent {
            return;
        }
        match self {
            Self::Unchanged => {}
            Self::Refreshed(session) => settings.hub_session = session,
            Self::Rejected => {
                settings.hub_session.clear();
                settings.hub_profile = None;
            }
        }
    }
}

/// Sends a JSON POST for `path` on Hub with `session` and parses a JSON response. Redirects are
/// not followed and non-2xx statuses are errors. The session update is reported even on errors.
pub async fn post<T: DeserializeOwned>(
    client: &Client,
    session: &str,
    path: &str,
    body: &impl Serialize,
) -> (SessionUpdate, Result<T, HubError>) {
    let response = match send(client, session, path, body).await {
        Ok(response) => response,
        Err(error) => return (SessionUpdate::Unchanged, Err(error)),
    };
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        log::info!("hub session rejected");
        return (SessionUpdate::Rejected, Err(HubError::SignedOut));
    }

    let update = match session_from_headers(response.headers()) {
        Ok(Some(refreshed)) => {
            log::info!("hub session refreshed");
            SessionUpdate::Refreshed(refreshed)
        }
        Ok(None) => SessionUpdate::Unchanged,
        Err(()) => {
            log::warn!("hub session refresh ignored: invalid cookie");
            SessionUpdate::Unchanged
        }
    };
    if !status.is_success() {
        return (update, Err(HubError::Status(status)));
    }
    let parsed = match response.bytes().await {
        Ok(body) => serde_json::from_slice(&body).map_err(|_| HubError::Response),
        Err(_) => Err(HubError::Response),
    };
    (update, parsed)
}

async fn send(
    client: &Client,
    session: &str,
    path: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response, HubError> {
    if session.is_empty() {
        return Err(HubError::SignedOut);
    }
    let url = hub_url(path).ok_or(HubError::Request)?;
    let body = serde_json::to_vec(body).map_err(|_| HubError::Request)?;
    client
        .post(url)
        .header(COOKIE, format!("session={session}"))
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| HubError::Request)
}

/// `path` on Hub, or `None` if it would leave the Hub origin (for example `//host`): the
/// session must never be sent elsewhere.
fn hub_url(path: &str) -> Option<Url> {
    let url = Url::parse(HUB_ORIGIN).ok()?.join(path).ok()?;
    (url.origin().ascii_serialization() == HUB_ORIGIN).then_some(url)
}

/// Agent token of a Hub hub. `token` is a secret: no `Debug`, never logged.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentToken {
    pub token: String,
    pub name: String,
    pub issued_at: u64,
}

/// Active agent tokens of a hub. Token values are unique; names are not (Hub allows empty,
/// duplicate and renamed names), so remember selections by value and use names for display.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentTokens(Vec<AgentToken>);

impl AgentTokens {
    /// The token with this value.
    #[must_use]
    pub fn get(&self, token: &str) -> Option<&AgentToken> {
        self.0.iter().find(|candidate| candidate.token == token)
    }

    /// The oldest token with exactly this name, so a later duplicate does not change the answer.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&AgentToken> {
        self.0
            .iter()
            .filter(|candidate| candidate.name == name)
            .min_by_key(|candidate| candidate.issued_at)
    }
}

/// Lists active agent tokens of `hub_id` (`agent_tokens`; recalled ones are separate in Hub).
pub async fn list_agent_tokens(
    client: &Client,
    session: &str,
    hub_id: &str,
) -> (SessionUpdate, Result<AgentTokens, HubError>) {
    #[derive(Serialize)]
    struct Request<'a> {
        hub_id: &'a str,
    }
    #[derive(Deserialize)]
    struct HubInfo {
        agent_tokens: AgentTokens,
    }

    let (update, info) = post::<HubInfo>(client, session, HUB_INFO_PATH, &Request { hub_id }).await;
    (update, info.map(|info| info.agent_tokens))
}

/// The `session` value from `Set-Cookie` headers: `None` without one, `Err` for an empty or
/// repeated one. Unparsable cookies are skipped.
fn session_from_headers(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut session = None;
    for header in headers.get_all(SET_COOKIE) {
        let Some(cookie) = header
            .to_str()
            .ok()
            .and_then(|header| Cookie::parse(header).ok())
        else {
            continue;
        };
        if cookie.name() != "session" {
            continue;
        }
        if cookie.value().is_empty() || session.replace(cookie.value().to_owned()).is_some() {
            return Err(());
        }
    }
    Ok(session)
}
