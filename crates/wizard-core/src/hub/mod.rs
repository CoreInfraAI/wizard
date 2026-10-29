//! Hub protocol shared by the GUI and CLI: HTTP client, authenticated requests with
//! session refresh, and agent tokens. Nothing here touches saved settings; callers apply
//! `SessionUpdate` themselves. Never include request URLs, headers, cookies, token values or
//! HTTP-client errors in diagnostics.

pub mod login;

use anyhow::anyhow;
use cookie::Cookie;
use core::time::Duration;
pub use reqwest::{Client, Url};
use reqwest::{
    RequestBuilder, StatusCode,
    header::{CONTENT_TYPE, COOKIE, HeaderMap, SET_COOKIE},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wizard_settings::Settings;

pub const HUB_ORIGIN: &str = "https://hub.coreinfra.ai";
const PROFILE_PATH: &str = "/hub/api/profile";
const HUB_INFO_PATH: &str = "/hub/api/hub/info";
const ISSUE_TOKEN_PATH: &str = "/hub/api/agent-token/issue";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Installs ring as the process-wide rustls provider; reqwest is built without one. Call once
/// at startup, before creating HTTP clients.
pub fn install_crypto_provider() -> anyhow::Result<()> {
    tokio_rustls::rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("rustls crypto provider already installed"))
}

/// HTTP client for Hub: Wizard's `User-Agent`, no redirects, no cookie store, `REQUEST_TIMEOUT`.
pub fn client() -> Result<Client, HubError> {
    Client::builder()
        .user_agent(wizard_proxy::user_agent())
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
                settings.active_hub_id.clear();
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
    let Ok(body) = serde_json::to_vec(body) else {
        return (SessionUpdate::Unchanged, Err(HubError::Request));
    };
    request(session, path, |url| {
        client
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
    })
    .await
}

/// Non-secret fields of the signed-in account (`GET /hub/api/profile`). The response also
/// carries account secrets; only `HubProfile` fields are parsed and nothing is logged.
pub async fn fetch_profile(
    client: &Client,
    session: &str,
) -> (SessionUpdate, Result<HubProfile, HubError>) {
    #[derive(Deserialize)]
    struct ProfileResponse {
        personal_hub_id: String,
        hubs: Vec<ProfileHub>,
        #[serde(default)]
        username: String,
        verified_email: Option<String>,
    }

    let (update, response) =
        request::<ProfileResponse>(session, PROFILE_PATH, |url| client.get(url)).await;
    let profile = response.and_then(|response| {
        if response.personal_hub_id.is_empty() {
            return Err(HubError::Response);
        }
        let hub = response
            .hubs
            .iter()
            .find(|hub| hub.hub_id == response.personal_hub_id)
            .ok_or(HubError::Response)?;
        let personal_hub_name = hub
            .display_name
            .as_ref()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| hub.hub_name.clone());
        Ok(HubProfile {
            personal_hub_id: response.personal_hub_id,
            personal_hub_name,
            hubs: response.hubs,
            username: response.username,
            verified_email: response.verified_email,
        })
    });
    (update, profile)
}

/// Sends the request built for `path` with `session`, as described for `post`.
async fn request<T: DeserializeOwned>(
    session: &str,
    path: &str,
    build: impl FnOnce(Url) -> RequestBuilder,
) -> (SessionUpdate, Result<T, HubError>) {
    if session.is_empty() {
        return (SessionUpdate::Unchanged, Err(HubError::SignedOut));
    }
    let Ok(url) = Url::parse(&format!("{HUB_ORIGIN}{path}")) else {
        return (SessionUpdate::Unchanged, Err(HubError::Request));
    };
    let Ok(response) = build(url)
        .header(COOKIE, format!("session={session}"))
        .send()
        .await
    else {
        return (SessionUpdate::Unchanged, Err(HubError::Request));
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

/// Non-secret Hub account details, constructed explicitly by `fetch_profile`.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct HubProfile {
    /// Default hub when no accessible hub has been selected.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub personal_hub_id: String,
    pub personal_hub_name: String,
    pub hubs: Vec<ProfileHub>,
    pub username: String,
    pub verified_email: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileHub {
    pub hub_id: String,
    pub hub_name: String,
    pub display_name: Option<String>,
    pub is_personal: bool,
}

impl HubProfile {
    /// Ignore a saved hub that is no longer in this account's memberships.
    #[must_use]
    pub fn active_hub_id<'a>(&'a self, selected: &'a str) -> &'a str {
        if self.hubs.iter().any(|hub| hub.hub_id == selected) {
            selected
        } else {
            &self.personal_hub_id
        }
    }
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

/// Issues a new agent token named `name` in `hub_id` and returns its value. Never log it.
pub async fn issue_agent_token(
    client: &Client,
    session: &str,
    hub_id: &str,
    name: &str,
) -> (SessionUpdate, Result<String, HubError>) {
    #[derive(Serialize)]
    struct Request<'a> {
        hub_id: &'a str,
        name: &'a str,
    }
    #[derive(Deserialize)]
    struct Issued {
        token: String,
    }

    let (update, issued) =
        post::<Issued>(client, session, ISSUE_TOKEN_PATH, &Request { hub_id, name }).await;
    let token = issued.and_then(|issued| {
        if issued.token.is_empty() {
            Err(HubError::Response)
        } else {
            Ok(issued.token)
        }
    });
    (update, token)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> HubProfile {
        HubProfile {
            personal_hub_id: "personal".into(),
            hubs: vec![ProfileHub {
                hub_id: "team".into(),
                hub_name: "Team".into(),
                display_name: None,
                is_personal: false,
            }],
            ..HubProfile::default()
        }
    }

    #[test]
    fn selected_hub_must_belong_to_profile() {
        let profile = profile();
        assert_eq!(profile.active_hub_id("team"), "team");
        assert_eq!(profile.active_hub_id(""), "personal");
        assert_eq!(profile.active_hub_id("removed-membership"), "personal");
    }

    #[test]
    fn rejected_session_forgets_hub_without_changing_agent_routing() {
        let mut settings = Settings {
            hub_session: "old-session".into(),
            active_hub_id: "team".into(),
            codex_proxy_enabled: Some("test-token".into()),
            ..Settings::default()
        };
        SessionUpdate::Rejected.apply(&mut settings, "old-session");
        assert!(settings.hub_session.is_empty());
        assert!(settings.active_hub_id.is_empty());
        assert_eq!(settings.codex_proxy_enabled.as_deref(), Some("test-token"));
    }

    #[test]
    fn stale_rejection_does_not_clear_new_account_or_hub() {
        let mut settings = Settings {
            hub_session: "new-session".into(),
            active_hub_id: "team".into(),
            ..Settings::default()
        };
        SessionUpdate::Rejected.apply(&mut settings, "old-session");
        assert_eq!(settings.hub_session, "new-session");
        assert_eq!(settings.active_hub_id, "team");
        SessionUpdate::Refreshed("refreshed-session".into()).apply(&mut settings, "new-session");
        assert_eq!(settings.active_hub_id, "team");
    }
}
