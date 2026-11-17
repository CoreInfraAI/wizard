//! Feed requests go directly through the Hub proxy, independently of the local daemon.

use anyhow::{Result, anyhow, bail};
use core::{error::Error, time::Duration};
use reqwest::{Client, Response, StatusCode};

pub(super) const TOKEN_REJECTED: &str =
    "Прокси CoreInfra AI Hub отклонил токен Wizard. Повторите вход в хаб и проверку обновлений.";

/// CONNECT failures have no HTTP response in reqwest. The dependency's tunnel error
/// is private; compare its fixed message without exposing arbitrary error contents.
pub(super) fn proxy_rejected_token(error: &reqwest::Error) -> bool {
    causes(error).any(|error| error.to_string() == "tunnel error: proxy authorization required")
}

fn causes<'a>(error: &'a (dyn Error + 'static)) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    core::iter::successors(Some(error), |error| (*error).source()).take(32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Token,
    Tls,
    Timeout,
    Connection,
    Other,
}

impl Failure {
    fn from_request(error: &reqwest::Error) -> Self {
        if proxy_rejected_token(error) {
            Self::Token
        } else if causes(error).any(<dyn Error>::is::<tokio_rustls::rustls::Error>) {
            Self::Tls
        } else if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() || error.is_request() {
            Self::Connection
        } else {
            Self::Other
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Token => TOKEN_REJECTED,
            Self::Tls => {
                "Не удалось установить защищённое соединение через CoreInfra AI Hub. Проверьте дату и время, VPN и сертификаты системы."
            }
            Self::Timeout => {
                "Проверка обновлений через CoreInfra AI Hub не дождалась ответа. Проверьте соединение и повторите попытку."
            }
            Self::Connection => {
                "Не удалось подключиться к серверу обновлений через CoreInfra AI Hub. Проверьте интернет-соединение и VPN."
            }
            Self::Other => {
                "Не удалось выполнить запрос обновлений через CoreInfra AI Hub. Повторите попытку."
            }
        }
    }

    const fn retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::Connection)
    }
}

/// Retry an idempotent feed request once after a transient connection/server failure.
/// Keep vendor-specific statuses (including Claude's 204/404) for the caller to interpret.
pub(super) async fn feed_response(client: &Client, url: &str) -> Result<Response> {
    let mut retried = false;
    loop {
        match client
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
        {
            Ok(response) if response.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
                bail!(TOKEN_REJECTED);
            }
            Ok(response)
                if !retried
                    && (response.status().is_server_error()
                        || response.status() == StatusCode::REQUEST_TIMEOUT) =>
            {
                log::warn!(
                    "desktop feed request will retry: status={}",
                    response.status().as_u16()
                );
            }
            Ok(response) => return Ok(response),
            Err(error) => {
                let failure = Failure::from_request(&error);
                log::warn!(
                    "desktop feed request failed: category={failure:?};{}",
                    wizard_proxy::diagnostics::summary(&error)
                );
                if retried || !failure.retryable() {
                    return Err(anyhow!(failure.message()));
                }
            }
        }
        retried = true;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "local HTTP fixtures")]

    extern crate alloc;

    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    async fn serve(
        responses: Vec<&'static str>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                    if socket.read_exact(&mut byte).await.is_err() {
                        break;
                    }
                    request.push(byte[0]);
                }
                let index = count
                    .fetch_add(1, Ordering::SeqCst)
                    .min(responses.len() - 1);
                let _ = socket.write_all(responses[index].as_bytes()).await;
            }
        });
        (format!("http://{address}"), requests, task)
    }

    #[tokio::test]
    async fn rejected_proxy_token_is_reported_without_retry_or_credentials() {
        let (url, count, server) = serve(vec![
            "HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
        ])
        .await;
        let client = Client::builder()
            .no_proxy()
            .proxy(
                reqwest::Proxy::all(url)
                    .unwrap()
                    .basic_auth("fixture", "secret-token"),
            )
            .build()
            .unwrap();
        let error = feed_response(&client, "https://feed.invalid/update?secret=fixture")
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), TOKEN_REJECTED);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!format!("{error:#}").contains("secret"));
        server.abort();
    }

    #[tokio::test]
    async fn temporary_server_failure_recovers_but_client_and_empty_responses_are_preserved() {
        let client = Client::builder().no_proxy().build().unwrap();
        let (url, count, server) = serve(vec![
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnew",
        ])
        .await;
        let response = feed_response(&client, &url).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "new");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.abort();
        for response in [
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 204 No Content\r\n\r\n",
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
        ] {
            let (url, count, server) = serve(vec![response]).await;
            let result = feed_response(&client, &url).await.unwrap();
            assert_eq!(result.status().as_u16().to_string(), &response[9..12]);
            assert_eq!(count.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }

    #[tokio::test]
    async fn disconnected_feed_retries_once_and_does_not_blame_the_token() {
        let (url, count, server) = serve(vec![""]).await;
        let client = Client::builder().no_proxy().build().unwrap();
        let error = feed_response(&client, &url).await.unwrap_err();
        assert_eq!(error.to_string(), Failure::Connection.message());
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.abort();
    }
}
