//! Unauthenticated loopback HTTP management of the standalone server.
//! These endpoints are not provided by GUI-owned standalone listeners.

use core::time::Duration;
use http_body_util::{BodyExt as _, Empty, Limited};
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;
use tokio::net::TcpStream;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE: usize = 1024;

/// Current runtime permission to proxy, not upstream health or token validity.
/// Both ports remain bound while the managed server is running.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    /// Empty for older responses without a version; current servers report their crate version.
    #[serde(default)]
    pub version: String,
    pub claude_desktop_enabled: bool,
    pub chatgpt_enabled: bool,
}

impl Status {
    pub(crate) fn enabled(&self) -> bool {
        self.claude_desktop_enabled || self.chatgpt_enabled
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadResponse {
    pub status: Status,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownResponse {
    pub pid: u32,
}

#[derive(Clone, Copy)]
pub(crate) enum Command {
    Status,
    Reload,
    Shutdown,
}

/// An unreachable server is an error, never a disabled-proxy report.
pub async fn status() -> io::Result<Status> {
    request(crate::CLAUDE_DESKTOP_ADDRESS, Command::Status).await
}

/// Once accepted, Reload completes even if this exchange times out or disconnects.
/// Do not automatically retry: after a timeout the result may be unknown.
pub async fn reload() -> io::Result<ReloadResponse> {
    request(crate::CLAUDE_DESKTOP_ADDRESS, Command::Reload).await
}

/// Request graceful exit without changing saved settings. The reply precedes
/// process exit; callers must wait for the returned PID separately. Do not retry
/// automatically after an ambiguous exchange failure.
pub async fn shutdown() -> io::Result<ShutdownResponse> {
    request(crate::CLAUDE_DESKTOP_ADDRESS, Command::Shutdown).await
}

pub(crate) async fn request<T: DeserializeOwned>(address: &str, command: Command) -> io::Result<T> {
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        // Connect directly, without HTTP_PROXY, redirects, or retries.
        let stream = TcpStream::connect(address).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .inspect_err(|error| crate::diagnostics::error("management HTTP handshake", error))
            .map_err(io::Error::other)?;
        let (method, path) = match command {
            Command::Status => ("GET", "/status"),
            Command::Reload => ("POST", "/reload"),
            Command::Shutdown => ("POST", "/shutdown"),
        };
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", address)
            .header("Connection", "close")
            .header("Content-Length", "0")
            .body(Empty::<Bytes>::new())
            .map_err(|_| io::Error::other("cannot construct management request"))?;
        let exchange = async {
            let response = sender
                .send_request(request)
                .await
                .inspect_err(|error| crate::diagnostics::error("management HTTP request", error))
                .map_err(io::Error::other)?;
            if response.status() != hyper::StatusCode::OK {
                log::error!(
                    "management HTTP request rejected: status={}",
                    response.status().as_u16()
                );
                return Err(io::Error::other("management HTTP request rejected"));
            }
            let bytes = Limited::new(response.into_body(), MAX_RESPONSE)
                .collect()
                .await
                .inspect_err(|error| {
                    crate::diagnostics::error("management response body", error.as_ref());
                })
                .map_err(io::Error::other)?
                .to_bytes();
            serde_json::from_slice(&bytes).map_err(|_| {
                log::error!("invalid management JSON response");
                io::Error::other("invalid management JSON response")
            })
        };
        let (connection, result) = tokio::join!(connection, exchange);
        if let Err(error) = connection {
            crate::diagnostics::error("management HTTP connection", &error);
        }
        result
    })
    .await
    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
}

#[cfg(test)]
mod tests {
    use super::Status;

    #[test]
    fn legacy_status_has_unknown_version_but_malformed_status_is_rejected() {
        let status: Status =
            serde_json::from_str(r#"{"claude_desktop_enabled":true,"chatgpt_enabled":false}"#)
                .expect("legacy status");
        assert!(status.version.is_empty());
        assert!(status.claude_desktop_enabled);
        assert!(!status.chatgpt_enabled);

        for response in [
            r#"{"version":null,"claude_desktop_enabled":true,"chatgpt_enabled":false}"#,
            r#"{"version":42,"claude_desktop_enabled":true,"chatgpt_enabled":false}"#,
            r#"{"claude_desktop_enabled":true}"#,
            r#"{"claude_desktop_enabled":true,"chatgpt_enabled":false,"unexpected":true}"#,
        ] {
            assert!(serde_json::from_str::<Status>(response).is_err());
        }
    }
}
