//! Browser login through Hub `app-login`. Login completes only after `/hub/api/profile`
//! accepts the new session and it is saved.
//! Never include request URLs, headers, cookies or HTTP-client errors in diagnostics;
//! log only `LoginError` variants.

use crate::{
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use alloc::sync::Arc;
use core::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    pin::pin,
    time::Duration,
};
use reqwest::{
    Client, Url,
    header::{COOKIE, HeaderMap, SET_COOKIE},
};
use serde::Serialize;
use std::{process::Stdio, sync::PoisonError};
use tauri::{Manager as _, async_runtime::JoinHandle};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
    time::Instant,
};
use wizard_core::settings::HubProfile;

// TEMP: local `cargo x ff-hub --https`; restore https://hub.coreinfra.ai before committing.
const HUB_ORIGIN: &str = "https://hub.local.coreinfra.ai:8443";
const AUTHORIZATION_URL: &str = "https://hub.local.coreinfra.ai:8443/app-login";
const CALLBACK_PARAMETER: &str = "callback_url";
const CALLBACK_PATH: &str = "/callback";
const LOGIN_LINK_PARAMETER: &str = "login_link";
const LOGIN_PATH: &str = "/hub/api/login";
const PROFILE_URL: &str = "https://hub.local.coreinfra.ai:8443/hub/api/profile";
const SUCCESS_URL: &str = "https://hub.local.coreinfra.ai:8443/app-login/success";
const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CALLBACK_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct HubAuth {
    /// Held by the single pending login attempt and by logout.
    operation: Arc<Mutex<()>>,
    /// Set while the attempt waits for the browser callback.
    waiting: std::sync::Mutex<Option<Waiting>>,
}

/// A login attempt waiting for the browser; another login press reopens its page.
struct Waiting {
    authorization: Url,
    deadline: Deadline,
}

/// Login deadline that a repeated login press extends.
#[derive(Clone)]
struct Deadline(Arc<std::sync::Mutex<Instant>>);

impl Deadline {
    fn new() -> Self {
        Self(Arc::new(std::sync::Mutex::new(
            Instant::now() + LOGIN_TIMEOUT,
        )))
    }

    fn get(&self) -> Instant {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn extend(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now() + LOGIN_TIMEOUT;
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct HubAccount {
    signed_in: bool,
    /// Saved at login; absent for sessions saved before profiles were stored.
    profile: Option<HubProfile>,
    pending: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginError {
    Listener,
    Browser,
    Timeout,
    Client,
    Exchange,
    Rejected,
    InvalidSession,
    MissingSession,
    Unverified,
    Save,
}

impl LoginError {
    /// User-facing error text.
    const fn message(self) -> &'static str {
        match self {
            Self::Listener => "Не удалось открыть локальный порт входа",
            Self::Browser => "Не удалось открыть браузер",
            Self::Timeout => "Время ожидания входа истекло. Попробуйте снова.",
            Self::Client => "Не удалось создать HTTP-клиент",
            Self::Exchange => "Не удалось обменять ссылку входа",
            Self::Rejected => "Hub отклонил ссылку входа",
            Self::InvalidSession => "Некорректная cookie сессии",
            Self::MissingSession => "Hub не вернул cookie сессии",
            Self::Unverified => "Hub не подтвердил сессию",
            Self::Save => "Не удалось сохранить сессию Wizard",
        }
    }
}

/// Registers login state; the initial `signed_in` comes from saved settings.
pub(crate) fn initialize(app: &tauri::AppHandle) -> anyhow::Result<()> {
    let settings = settings::get_state(app)?;
    app.manage(HubAuth::default());
    app.manage(RevisionSignal::new(HubAccount {
        signed_in: !settings.hub_session.is_empty(),
        profile: settings.hub_profile,
        pending: false,
        error: None,
    }));
    Ok(())
}

/// Returns the account state once it differs from `last_revision`.
#[tauri::command]
pub(crate) async fn get_hub_account_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<HubAccount>>,
) -> Result<RevisionSnapshot<HubAccount>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить состояние входа".into())
}

/// Starts a background login attempt unless a session is saved. While an attempt waits for the
/// browser, reopens its page and extends its deadline instead.
#[tauri::command]
pub(crate) async fn login_hub_account(app: tauri::AppHandle) -> Result<(), String> {
    let auth = app.state::<HubAuth>();
    let Ok(guard) = Arc::clone(&auth.operation).try_lock_owned() else {
        return reopen(&auth).await;
    };
    if signed_in(&app).map_err(|_| "Настройки недоступны")? {
        return Ok(());
    }

    publish(&app, true, None);
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let result = login(&app).await;
        match result {
            Ok(()) => log::info!("hub login succeeded"),
            Err(error) => log::warn!("hub login failed: {error:?}"),
        }
        publish(&app, false, result.err());
    });
    Ok(())
}

/// Reopens the waiting attempt's Hub page and extends its deadline.
async fn reopen(auth: &HubAuth) -> Result<(), String> {
    let authorization = {
        let waiting = auth.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // No waiting attempt: the callback arrived and the session is being exchanged.
        let waiting = waiting.as_ref().ok_or("Вход уже выполняется")?;
        waiting.deadline.extend();
        waiting.authorization.clone()
    };
    log::info!("hub login page reopened");
    open_browser(authorization.as_str())
        .await
        .map_err(|error| error.message().to_owned())
}

/// Clears Wizard's saved session; browser sessions are untouched.
#[tauri::command]
pub(crate) async fn logout_hub_account(app: tauri::AppHandle) -> Result<(), String> {
    let _guard = Arc::clone(&app.state::<HubAuth>().operation)
        .try_lock_owned()
        .map_err(|_| "Дождитесь завершения попытки входа")?;
    settings::update_state(&app, |s| {
        s.hub_session.clear();
        s.hub_profile = None;
    })
    .await
    .map_err(|_| "Не удалось удалить сессию Wizard")?;
    log::info!("hub logout");
    publish(&app, false, None);
    Ok(())
}

/// Whether a Hub session is saved.
fn signed_in(app: &tauri::AppHandle) -> anyhow::Result<bool> {
    Ok(!settings::get_state(app)?.hub_session.is_empty())
}

/// Publishes the account state; `signed_in` and `profile` always come from saved settings.
fn publish(app: &tauri::AppHandle, pending: bool, error: Option<LoginError>) {
    let (signed_in, profile) = settings::get_state(app).map_or((false, None), |s| {
        (!s.hub_session.is_empty(), s.hub_profile)
    });
    app.state::<RevisionSignal<HubAccount>>()
        .notify(HubAccount {
            signed_in,
            profile,
            pending,
            error: error.map(|error| error.message().to_owned()),
        });
}

/// Runs one login: callback, browser, exchange, profile check, save, browser reply.
/// Waiting and network steps share one deadline. The settings write is never cancelled.
async fn login(app: &tauri::AppHandle) -> Result<(), LoginError> {
    let deadline = Deadline::new();
    let mut server = CallbackServer::bind().await?;
    let authorization = server.authorization_url()?;
    let client = login_client()?;

    let waiting = &app.state::<HubAuth>().waiting;
    *waiting.lock().unwrap_or_else(PoisonError::into_inner) = Some(Waiting {
        authorization: authorization.clone(),
        deadline: deadline.clone(),
    });
    let callback = async {
        before(&deadline, open_browser(authorization.as_str())).await?;
        before(&deadline, server.next_link()).await
    }
    .await;
    *waiting.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let (mut stream, link) = callback?;
    drop(server);

    let session = before(&deadline, async {
        let session = exchange(&client, link).await?;
        let profile = fetch_profile(&client, PROFILE_URL, &session).await?;
        Ok((session, profile))
    });
    let (session, profile) = match session.await {
        Ok(session) => session,
        Err(error) => {
            reply(&mut stream, &LOGIN_FAILED).await;
            return Err(error);
        }
    };
    if settings::update_state(app, move |s| {
        s.hub_session = session;
        s.hub_profile = Some(profile);
    })
    .await
    .is_err()
    {
        reply(&mut stream, &SAVE_FAILED).await;
        return Err(LoginError::Save);
    }
    reply(&mut stream, &SIGNED_IN).await;
    Ok(())
}

/// Runs `future`, failing with `Timeout` once `deadline` passes without being extended.
async fn before<T>(
    deadline: &Deadline,
    future: impl Future<Output = Result<T, LoginError>>,
) -> Result<T, LoginError> {
    let mut future = pin!(future);
    loop {
        let at = deadline.get();
        if let Ok(result) = tokio::time::timeout_at(at, &mut future).await {
            return result;
        }
        if deadline.get() <= at {
            return Err(LoginError::Timeout);
        }
    }
}

/// Loopback listener for the browser callback. Each connection is read in its own task, so an
/// idle (for example, speculative) browser connection cannot delay the real callback.
struct CallbackServer {
    address: SocketAddr,
    links: mpsc::Receiver<(TcpStream, Url)>,
    acceptor: JoinHandle<()>,
}

impl CallbackServer {
    /// Listens on an ephemeral loopback port and starts accepting connections.
    async fn bind() -> Result<Self, LoginError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| LoginError::Listener)?;
        let address = listener.local_addr().map_err(|_| LoginError::Listener)?;
        let (sender, links) = mpsc::channel(1);
        let acceptor = tauri::async_runtime::spawn(accept_callbacks(listener, address, sender));
        Ok(Self {
            address,
            links,
            acceptor,
        })
    }

    /// Hub `app-login` URL carrying this server's callback URL.
    fn authorization_url(&self) -> Result<Url, LoginError> {
        let mut url = Url::parse(AUTHORIZATION_URL).map_err(|_| LoginError::Browser)?;
        url.query_pairs_mut().append_pair(
            CALLBACK_PARAMETER,
            &format!("http://{}{CALLBACK_PATH}", self.address),
        );
        Ok(url)
    }

    /// Returns the first valid callback with its unanswered connection.
    async fn next_link(&mut self) -> Result<(TcpStream, Url), LoginError> {
        // `None` means the listener failed and no pending connection is left.
        self.links.recv().await.ok_or(LoginError::Listener)
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        // Pending readers end on their own within REQUEST_TIMEOUT.
        self.acceptor.abort();
    }
}

/// Reads each connection in its own task and forwards valid callbacks;
/// invalid requests get `INVALID_CALLBACK`.
async fn accept_callbacks(
    listener: TcpListener,
    address: SocketAddr,
    links: mpsc::Sender<(TcpStream, Url)>,
) {
    let host = address.to_string();
    while let Ok((mut stream, _)) = listener.accept().await {
        let host = host.clone();
        let links = links.clone();
        tauri::async_runtime::spawn(async move {
            match read_callback(&mut stream, &host).await {
                Some(link) => {
                    let _ = links.send((stream, link)).await;
                }
                None => reply(&mut stream, &INVALID_CALLBACK).await,
            }
        });
    }
}

/// Reads one request within `REQUEST_TIMEOUT` and extracts its login link.
async fn read_callback(stream: &mut TcpStream, host: &str) -> Option<Url> {
    let head = tokio::time::timeout(REQUEST_TIMEOUT, read_request_head(stream))
        .await
        .ok()??;
    parse_callback(&head, host)
}

/// Reads the request line and headers, without the terminating empty line.
async fn read_request_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let size = stream.read(&mut buffer).await.ok()?;
        if size == 0 {
            return None;
        }
        let searched = head.len().saturating_sub(3);
        head.extend_from_slice(&buffer[..size]);
        if let Some(end) = head[searched..]
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
        {
            head.truncate(searched + end);
            return (head.len() <= MAX_CALLBACK_BYTES).then_some(head);
        }
        if head.len() > MAX_CALLBACK_BYTES {
            return None;
        }
    }
}

/// Accepts only `GET <target> HTTP/1.1` with exactly one matching `Host` header.
fn parse_callback(head: &[u8], host: &str) -> Option<Url> {
    let head = core::str::from_utf8(head).ok()?;
    let mut lines = head.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let (Some("GET"), Some(target), Some("HTTP/1.1"), None) = (
        request.next(),
        request.next(),
        request.next(),
        request.next(),
    ) else {
        return None;
    };

    let mut hosts = lines.filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host").then_some(value.trim())
    });
    if hosts.next() != Some(host) || hosts.next().is_some() {
        return None;
    }
    callback_link(target)
}

/// Extracts and validates the single `login_link` from a `/callback?…` target.
fn callback_link(target: &str) -> Option<Url> {
    if !target.strip_prefix(CALLBACK_PATH)?.starts_with('?') {
        return None;
    }
    // Only the path and query come from the request; the origin is fixed.
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != CALLBACK_PATH || url.fragment().is_some() {
        return None;
    }
    let mut pairs = url.query_pairs();
    let (Some((name, link)), None) = (pairs.next(), pairs.next()) else {
        return None;
    };
    if name != LOGIN_LINK_PARAMETER {
        return None;
    }
    let link = Url::parse(&link).ok()?;
    is_login_link(&link).then_some(link)
}

/// Whether `link` is exactly `https://hub.coreinfra.ai/hub/api/login?key=<non-empty>`.
fn is_login_link(link: &Url) -> bool {
    let mut pairs = link.query_pairs();
    link.origin().ascii_serialization() == HUB_ORIGIN
        && link.path() == LOGIN_PATH
        && link.username().is_empty()
        && link.password().is_none()
        && link.fragment().is_none()
        && matches!(
            (pairs.next(), pairs.next()),
            (Some((name, key)), None) if name == "key" && !key.is_empty()
        )
}

/// Opens `url` in the default browser with the platform launcher.
async fn open_browser(url: &str) -> Result<(), LoginError> {
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = tokio::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    let status = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|_| LoginError::Browser)?;
    if !status.success() {
        return Err(LoginError::Browser);
    }
    Ok(())
}

/// HTTP client for login requests: no redirects, no cookie store, `REQUEST_TIMEOUT`.
fn login_client() -> Result<Client, LoginError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| LoginError::Client)
}

/// Exchanges the one-time login link for the `session` cookie it sets.
async fn exchange(client: &Client, link: Url) -> Result<String, LoginError> {
    // Redirects are disabled: the login response itself sets the session cookie.
    let response = client
        .get(link)
        .send()
        .await
        .map_err(|_| LoginError::Exchange)?;
    let status = response.status();
    if !status.is_success() && !status.is_redirection() {
        return Err(LoginError::Rejected);
    }
    session_from_headers(response.headers())
}

/// Confirms that `/hub/api/profile` accepts `session` and returns its displayable fields.
/// A redirect does not prove success: Hub also reports errors that way. The response also
/// carries account secrets; only `HubProfile` fields are parsed and nothing is logged.
async fn fetch_profile(
    client: &Client,
    profile: &str,
    session: &str,
) -> Result<HubProfile, LoginError> {
    let response = client
        .get(profile)
        .header(COOKIE, format!("session={session}"))
        .send()
        .await
        .map_err(|_| LoginError::Unverified)?;
    if !response.status().is_success() {
        return Err(LoginError::Unverified);
    }
    let body = response.bytes().await.map_err(|_| LoginError::Unverified)?;
    serde_json::from_slice(&body).map_err(|_| LoginError::Unverified)
}

/// Extracts the session from `Set-Cookie` headers.
/// Requires exactly one valid `session` cookie.
fn session_from_headers(headers: &HeaderMap) -> Result<String, LoginError> {
    let mut session = None;
    for header in headers.get_all(SET_COOKIE) {
        let header = header.to_str().map_err(|_| LoginError::InvalidSession)?;
        let Some(value) = session_cookie(header)? else {
            continue;
        };
        if session.replace(value).is_some() {
            return Err(LoginError::InvalidSession);
        }
    }
    session.map(str::to_owned).ok_or(LoginError::MissingSession)
}

/// Returns the value of a `session` cookie header, or `None` for other cookies.
fn session_cookie(header: &str) -> Result<Option<&str>, LoginError> {
    let Some((name, value)) = header
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
    else {
        return Ok(None);
    };
    if name.trim() != "session" {
        return Ok(None);
    }
    let value = value.trim();
    // RFC 6265 cookie-octet; rejects header injection and extra cookies.
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|b| matches!(b, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e));
    if !valid {
        return Err(LoginError::InvalidSession);
    }
    Ok(Some(value))
}

struct Response {
    status: &'static str,
    location: Option<&'static str>,
    body: &'static str,
}

const SIGNED_IN: Response = Response {
    status: "303 See Other",
    location: Some(SUCCESS_URL),
    body: "",
};
const INVALID_CALLBACK: Response = Response {
    status: "400 Bad Request",
    location: None,
    body: "Invalid login callback.",
};
const LOGIN_FAILED: Response = Response {
    status: "502 Bad Gateway",
    location: None,
    body: "Login failed. Return to Wizard and try again.",
};
const SAVE_FAILED: Response = Response {
    status: "500 Internal Server Error",
    location: None,
    body: "Could not save session. Return to Wizard.",
};

/// Sends `response` and closes the connection; write failures are ignored.
async fn reply(stream: &mut TcpStream, response: &Response) {
    let location = response
        .location
        .map_or_else(String::new, |url| format!("Location: {url}\r\n"));
    let message = format!(
        "HTTP/1.1 {}\r\n{location}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{}",
        response.status,
        response.body.len(),
        response.body,
    );
    let _ = tokio::time::timeout(REQUEST_TIMEOUT, async {
        stream.write_all(message.as_bytes()).await?;
        stream.shutdown().await
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_link_is_restricted_to_hub() {
        for url in [
            "http://hub.coreinfra.ai/hub/api/login?key=test",
            "https://example.com/hub/api/login?key=test",
            "https://hub.coreinfra.ai:444/hub/api/login?key=test",
            "https://user@hub.coreinfra.ai/hub/api/login?key=test",
            "https://hub.coreinfra.ai/login?key=test",
            "https://hub.coreinfra.ai/hub/api/login?key=",
            "https://hub.coreinfra.ai/hub/api/login?key=test&key=other",
            "https://hub.coreinfra.ai/hub/api/login?key=test#fragment",
        ] {
            assert!(!is_login_link(&Url::parse(url).expect("fixture URL")));
        }

        // TEMP: local Hub origin; restore hub.coreinfra.ai together with the constants.
        let link = callback_link(
            "/callback?login_link=https%3A%2F%2Fhub.local.coreinfra.ai%3A8443%2Fhub%2Fapi%2Flogin%3Fkey%3Dtest",
        );
        assert_eq!(
            link.map(String::from).as_deref(),
            Some("https://hub.local.coreinfra.ai:8443/hub/api/login?key=test")
        );
    }

    /// Login answers with a redirect that sets the cookie; only `/profile` decides success.
    #[test]
    fn exchange_keeps_redirect_cookie_and_profile_decides() {
        // Tests skip application startup; another test may already have installed it.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        tauri::async_runtime::block_on(async {
            let client = login_client().expect("HTTP client");
            // The profile also carries secrets such as `login_key`; only display fields are kept.
            let profile_body = r#"{"username":"test-user","verified_email":"user@example.com","login_key":"secret","login_link":"secret"}"#;
            let account = HubProfile {
                username: "test-user".to_owned(),
                verified_email: Some("user@example.com".to_owned()),
            };
            for (profile_status, expected) in [
                ("200 OK", Ok(account)),
                ("303 See Other", Err(LoginError::Unverified)),
            ] {
                let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                    .await
                    .expect("test listener");
                let address = listener.local_addr().expect("test address");
                let responses = [
                    ("303 See Other\r\nSet-Cookie: session=test; HttpOnly", ""),
                    (profile_status, profile_body),
                ];
                // `Location` points to a closed port: a followed redirect fails the exchange.
                let server = tauri::async_runtime::spawn(async move {
                    let mut heads = Vec::new();
                    for (status, body) in responses {
                        let (mut stream, _) = listener.accept().await.expect("test connection");
                        let head = read_request_head(&mut stream).await.expect("test request");
                        heads.push(String::from_utf8(head).expect("test request head"));
                        let response = format!(
                            "HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:1/login\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream
                            .write_all(response.as_bytes())
                            .await
                            .expect("test response");
                    }
                    heads
                });

                let link = Url::parse(&format!("http://{address}/hub/api/login?key=test"))
                    .expect("test URL");
                let session = exchange(&client, link).await.expect("session");
                assert_eq!(session, "test");
                let profile = format!("http://{address}/hub/api/profile");
                assert_eq!(fetch_profile(&client, &profile, &session).await, expected);

                let heads = server.await.expect("test server");
                let profile = heads[1].to_ascii_lowercase();
                assert!(profile.starts_with("get /hub/api/profile http/1.1\r\n"));
                assert!(
                    profile
                        .split("\r\n")
                        .any(|line| line == "cookie: session=test")
                );
            }
        });
    }
}
