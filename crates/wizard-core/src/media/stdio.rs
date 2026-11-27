//! Claude Desktop launches this stdio connector. Only Wizard receives Hub credentials.
use anyhow::{Context as _, Result, ensure};
use core::time::Duration;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerConfig,
    },
    service::RequestContext,
    transport::{async_rw::AsyncRwTransport, io::stdio_pipes},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::path::PathBuf;

pub const ARG: &str = "--media-mcp-stdio";
/// Written into Claude Desktop configurations by 1.0.10; still accepted until they are applied.
pub const LEGACY_ARG: &str = "--video-mcp-stdio";

/// Whether `arg` selects this connector.
#[must_use]
pub fn is_arg(arg: &std::ffi::OsStr) -> bool {
    arg == ARG || arg == LEGACY_ARG
}

// Shares the Code client's owner-only HTTP headers file. This is a loopback credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Headers {
    #[serde(rename = "Authorization")]
    authorization: String,
}

struct Connector {
    headers: PathBuf,
    port: u16,
    client: reqwest::Client,
}

impl Connector {
    async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, ErrorData> {
        // Read for every call so an open Claude session follows a restarted Wizard.
        let bytes = tokio::fs::read(&self.headers).await.map_err(|_| {
            ErrorData::internal_error("Open CoreInfra Wizard and enable media models", None)
        })?;
        let headers: Headers = serde_json::from_slice(&bytes)
            .map_err(|_| ErrorData::internal_error("Invalid Wizard media connection", None))?;
        let request = self
            .client
            .post(format!("http://127.0.0.1:{}/mcp", self.port))
            .header(reqwest::header::AUTHORIZATION, headers.authorization)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "2025-11-25")
            .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string());
        // Each POST is independent. Never replay a paid submission on a transport error.
        // CallToolResult::structured repeats the payload in both content and structuredContent,
        // so allow up to MAX_REQUEST_BYTES (matching AsyncRwTransport) on loopback replies.
        let mut reply = super::read_json_limit(request, super::MAX_REQUEST_BYTES)
            .await
            .map_err(|_| {
                ErrorData::internal_error(
                    "Wizard media request failed; an interrupted submission may have been accepted",
                    None,
                )
            })?;
        if let Some(error) = reply.get_mut("error")
            && let Ok(error) = serde_json::from_value(error.take())
        {
            return Err(error);
        }
        let result = reply
            .get_mut("result")
            .map_or(serde_json::Value::Null, serde_json::Value::take);
        serde_json::from_value(result)
            .map_err(|_| ErrorData::internal_error("Invalid Wizard media response", None))
    }
}

impl ServerHandler for Connector {
    fn get_info(&self) -> ServerConfig {
        super::server::server_info()
    }

    async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.request("tools/list", params.unwrap_or_default()).await
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Dropping the loopback request on cancellation lets Wizard skip a paid submission
        // that has not started yet.
        tokio::select! {
            () = context.ct.cancelled() => Err(ErrorData::internal_error("Wizard media request was cancelled", None)),
            result = self.request::<CallToolResult>("tools/call", params) => result.map(Into::into),
        }
    }
}

pub async fn run(port: u16, headers: PathBuf) -> Result<()> {
    ensure!(
        port != 0 && headers.is_absolute(),
        "Expected an absolute media connection path"
    );
    crate::hub::install_crypto_provider()?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(240))
        .build()?;
    let (input, output) = stdio_pipes();
    Connector {
        headers,
        port,
        client,
    }
    .serve(AsyncRwTransport::new(input, output).with_max_line_length(super::MAX_REQUEST_BYTES))
    .await
    .context("Could not initialize Claude media MCP")?
    .waiting()
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::Full;
    use hyper::{Response, body::Bytes, server::conn::http1, service::service_fn};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn forwards_structured_tool_replies_larger_than_hub_json_limit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let headers = dir.path().join("media-claude-desktop-headers.json");
        std::fs::write(&headers, r#"{"Authorization":"Bearer wizard-media-test"}"#)?;
        // A ~5 MiB transcript wrapped by CallToolResult::structured exceeds MAX_RESPONSE_BYTES (8 MiB).
        let text = "a".repeat(5 * 1024 * 1024);
        let structured = CallToolResult::structured(json!({"text": text}));
        let envelope = json!({"jsonrpc":"2.0","id":1,"result":structured}).to_string();
        assert!(envelope.len() > super::super::MAX_RESPONSE_BYTES);
        let payload = Bytes::from(envelope);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            let handler = service_fn(move |_| {
                let body = payload.clone();
                async move {
                    Ok::<_, core::convert::Infallible>(
                        Response::builder()
                            .header("content-type", "application/json")
                            .body(Full::new(body))
                            .expect("response"),
                    )
                }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(socket), handler)
                .await;
        });

        let connector = Connector {
            headers,
            port,
            client: reqwest::Client::builder().no_proxy().build()?,
        };
        let reply: CallToolResult = connector
            .request(
                "tools/call",
                json!({"name":"transcription_create","arguments":{}}),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        assert_eq!(
            reply
                .structured_content
                .as_ref()
                .and_then(|v| v["text"].as_str())
                .map(str::len),
            Some(5 * 1024 * 1024)
        );
        drop(connector);
        server.await?;
        Ok(())
    }
}
