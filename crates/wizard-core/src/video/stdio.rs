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

pub const ARG: &str = "--video-mcp-stdio";

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
            ErrorData::internal_error("Open CoreInfra Wizard and enable video models", None)
        })?;
        let headers: Headers = serde_json::from_slice(&bytes)
            .map_err(|_| ErrorData::internal_error("Invalid Wizard video connection", None))?;
        let request = self
            .client
            .post(format!("http://127.0.0.1:{}/mcp", self.port))
            .header(reqwest::header::AUTHORIZATION, headers.authorization)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", "2025-11-25")
            .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string());
        // Each POST is independent. Never replay a paid submission on a transport error.
        let mut reply = super::read_json(request).await.map_err(|_| {
            ErrorData::internal_error(
                "Wizard video request failed; an interrupted submission may have been accepted",
                None,
            )
        })?;
        if let Some(error) = reply.get_mut("error")
            && let Ok(error) = serde_json::from_value(error.take())
        {
            return Err(error);
        }
        serde_json::from_value(reply["result"].take())
            .map_err(|_| ErrorData::internal_error("Invalid Wizard video response", None))
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
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.request::<CallToolResult>("tools/call", params)
            .await
            .map(Into::into)
    }
}

pub async fn run(port: u16, headers: PathBuf) -> Result<()> {
    ensure!(
        port != 0 && headers.is_absolute(),
        "Expected an absolute video connection path"
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
    .context("Could not initialize Claude video MCP")?
    .waiting()
    .await?;
    Ok(())
}
