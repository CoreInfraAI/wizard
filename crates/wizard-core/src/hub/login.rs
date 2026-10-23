//! Browser login through Hub `app-login`, split into steps so the GUI and CLI can wrap them
//! with their own waiting and saving:
//!
//! `CallbackServer::bind` → `authorization_url` → `open_browser` → `next_link` →
//! `Callback::sign_in` → save the session and profile → `Callback::reply`.
//!
//! Login completes only after `/hub/api/profile` accepts the new session. There is no state or
//! PKCE by user decision; callback substitution remains a known risk.

use super::{HUB_ORIGIN, HubProfile, session_from_headers};
use core::{
    fmt,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use reqwest::{Client, Url};
use std::process::Stdio;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
};

const AUTHORIZATION_PATH: &str = "/app-login";
const SUCCESS_PATH: &str = "/app-login/success";
const CALLBACK_PARAMETER: &str = "callback_url";
const CALLBACK_PATH: &str = "/callback";
const LOGIN_LINK_PARAMETER: &str = "login_link";
const LOGIN_PATH: &str = "/hub/api/login";
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginError {
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
    #[must_use]
    pub const fn message(self) -> &'static str {
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

impl fmt::Display for LoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for LoginError {}

/// Loopback listener for the browser callback. Each connection is read in its own task, so an
/// idle (for example, speculative) browser connection cannot delay the real callback.
pub struct CallbackServer {
    address: SocketAddr,
    links: mpsc::Receiver<Callback>,
    acceptor: JoinHandle<()>,
}

impl CallbackServer {
    /// Listens on an ephemeral `127.0.0.1` port and starts accepting connections.
    pub async fn bind() -> Result<Self, LoginError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| LoginError::Listener)?;
        let address = listener.local_addr().map_err(|_| LoginError::Listener)?;
        let (sender, links) = mpsc::channel(1);
        let acceptor = tokio::spawn(accept_callbacks(listener, sender));
        Ok(Self {
            address,
            links,
            acceptor,
        })
    }

    /// Hub `app-login` URL carrying this server's callback URL. Contains no secrets.
    #[must_use]
    pub fn authorization_url(&self) -> Url {
        let mut url = Url::parse(&format!("{HUB_ORIGIN}{AUTHORIZATION_PATH}"))
            .expect("Hub authorization URL is valid");
        url.query_pairs_mut().append_pair(
            CALLBACK_PARAMETER,
            &format!("http://{}{CALLBACK_PATH}", self.address),
        );
        url
    }

    /// Waits for the first valid callback; its connection stays open for `Callback::reply`.
    pub async fn next_link(&mut self) -> Result<Callback, LoginError> {
        // `None` means the listener failed and no pending connection is left.
        self.links.recv().await.ok_or(LoginError::Listener)
    }
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        // Pending readers end on their own within CALLBACK_READ_TIMEOUT.
        self.acceptor.abort();
    }
}

/// A received callback: the validated one-time login link and the browser's open connection.
pub struct Callback {
    stream: TcpStream,
    link: Url,
}

/// Browser page for the pending callback.
#[derive(Clone, Copy)]
pub enum Reply {
    /// Redirect to Hub's success page. Only after the session is saved.
    SignedIn,
    Failed,
}

impl Callback {
    /// Exchanges the login link for a session and confirms it with `/hub/api/profile`.
    pub async fn sign_in(&self, client: &Client) -> Result<(String, HubProfile), LoginError> {
        let session = exchange(client, self.link.clone()).await?;
        let (_, profile) = super::fetch_profile(client, &session).await;
        let profile = profile.map_err(|_| LoginError::Unverified)?;
        Ok((session, profile))
    }

    /// Answers the browser and closes the connection; write failures are ignored.
    pub async fn reply(mut self, reply: Reply) {
        let success = format!("{HUB_ORIGIN}{SUCCESS_PATH}");
        let (status, location, body) = match reply {
            Reply::SignedIn => ("303 See Other", Some(success.as_str()), ""),
            Reply::Failed => (
                "502 Bad Gateway",
                None,
                "Login failed. Return to Wizard and try again.",
            ),
        };
        respond(&mut self.stream, status, location, body).await;
    }
}

/// Reads each connection in its own task and forwards valid callbacks; invalid requests get 400.
async fn accept_callbacks(listener: TcpListener, links: mpsc::Sender<Callback>) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let links = links.clone();
        tokio::spawn(async move {
            match read_callback(&mut stream).await {
                Some(link) => {
                    let _ = links.send(Callback { stream, link }).await;
                }
                None => {
                    respond(
                        &mut stream,
                        "400 Bad Request",
                        None,
                        "Invalid login callback.",
                    )
                    .await;
                }
            }
        });
    }
}

/// Reads one request within `CALLBACK_READ_TIMEOUT` and extracts its login link.
async fn read_callback(stream: &mut TcpStream) -> Option<Url> {
    let head = tokio::time::timeout(CALLBACK_READ_TIMEOUT, read_request_head(stream))
        .await
        .ok()??;
    parse_callback(&head)
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
            return Some(head);
        }
    }
}

/// Accepts only `GET <target> HTTP/1.1` with exactly one loopback `Host` header.
fn parse_callback(head: &[u8]) -> Option<Url> {
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
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return None;
    };
    // Only a loopback IP literal with a port (`127.0.0.1:port`, `[::1]:port`). Names such as
    // `localhost` are rejected: a DNS name could point a foreign page at the listener.
    if !host
        .parse::<SocketAddr>()
        .is_ok_and(|address| address.ip().is_loopback())
    {
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

/// Whether `link` is exactly `<HUB_ORIGIN>/hub/api/login?key=<non-empty>`.
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
pub async fn open_browser(url: &Url) -> Result<(), LoginError> {
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
        .arg(url.as_str())
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
        .map_err(|()| LoginError::InvalidSession)?
        .ok_or(LoginError::MissingSession)
}

/// Sends a plain-text response and closes the connection; write failures are ignored.
async fn respond(stream: &mut TcpStream, status: &str, location: Option<&str>, body: &str) {
    let location = location.map_or_else(String::new, |url| format!("Location: {url}\r\n"));
    let message = format!(
        "HTTP/1.1 {status}\r\n{location}Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len(),
    );
    let _ = tokio::time::timeout(CALLBACK_READ_TIMEOUT, async {
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

        let link = callback_link(
            "/callback?login_link=https%3A%2F%2Fhub.coreinfra.ai%2Fhub%2Fapi%2Flogin%3Fkey%3Dtest",
        );
        assert_eq!(
            link.map(String::from).as_deref(),
            Some("https://hub.coreinfra.ai/hub/api/login?key=test")
        );
    }
}
