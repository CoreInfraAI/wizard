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

/// Include the proxy revision so incompatible development builds with the same
/// application version are replaced once instead of keeping an old daemon alive.
pub const DAEMON_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+proxy.4");

/// Current runtime permission to proxy, not upstream health or token validity.
/// Both ports remain bound while the managed server is running.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub version: String,
    pub enabled: bool,
    // Keep legacy wire field names so 1.0.10 CLI and daemon remain mutually compatible.
    #[serde(default, rename = "video_running", alias = "media_running")]
    pub media_running: bool,
    #[serde(default, rename = "video_error", alias = "media_error")]
    pub media_error: Option<String>,
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
    request(crate::ADDRESS, Command::Status).await
}

/// Once accepted, Reload completes even if this exchange times out or disconnects.
/// Do not automatically retry: after a timeout the result may be unknown.
pub async fn reload() -> io::Result<ReloadResponse> {
    request(crate::ADDRESS, Command::Reload).await
}

/// Request graceful exit without changing saved settings. The reply precedes
/// process exit; callers must wait for the returned PID separately. Do not retry
/// automatically after an ambiguous exchange failure.
pub async fn shutdown() -> io::Result<ShutdownResponse> {
    request(crate::ADDRESS, Command::Shutdown).await
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
    fn malformed_status_is_rejected() {
        for response in [
            r#"{"version":null,"enabled":true}"#,
            r#"{"version":42,"enabled":true}"#,
            r#"{"enabled":true}"#,
            r#"{"version":"test"}"#,
            r#"{"version":"test","enabled":"true"}"#,
            r#"{"version":"test","enabled":true,"unexpected":true}"#,
        ] {
            assert!(serde_json::from_str::<Status>(response).is_err());
        }
    }

    #[test]
    fn status_is_wire_compatible_with_1_0_10_in_both_directions() -> serde_json::Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyStatus {
            version: String,
            enabled: bool,
            #[serde(default)]
            video_running: bool,
            #[serde(default)]
            video_error: Option<String>,
        }
        let legacy: Status = serde_json::from_str(
            r#"{"version":"1.0.10+proxy.3","enabled":false,"video_running":true,"video_error":"stopped"}"#,
        )?;
        assert!(legacy.media_running);
        assert_eq!(legacy.media_error.as_deref(), Some("stopped"));
        let emitted = serde_json::to_string(&legacy)?;
        let decoded: LegacyStatus = serde_json::from_str(&emitted)?;
        assert_eq!(decoded.version, "1.0.10+proxy.3");
        assert!(!decoded.enabled && decoded.video_running);
        assert_eq!(decoded.video_error.as_deref(), Some("stopped"));
        let renamed: Status = serde_json::from_str(
            r#"{"version":"1.0.11+proxy.4","enabled":true,"media_running":true,"media_error":null}"#,
        )?;
        assert!(renamed.media_running && renamed.media_error.is_none());
        Ok(())
    }
}
