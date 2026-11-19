use super::{Create, GetResult, VideoHub};
use alloc::sync::Arc;
use anyhow::{Context as _, Result, bail, ensure};
use core::{convert::Infallible, future::Future};
use http_body_util::{BodyExt as _, Empty, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::AUTHORIZATION,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::common::schema_for_type,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinSet};
use wizard_settings::VideoConnection;

/// The GUI owns the grant and the signed-in Hub account. No upstream key is cached here.
pub trait VideoAccess: Send + Sync + 'static {
    fn connection(&self) -> Option<VideoConnection>;
    fn token(&self, connection: &VideoConnection) -> impl Future<Output = Result<String>> + Send;
}

struct Tools<A> {
    hub: VideoHub,
    access: Arc<A>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

impl<A: VideoAccess> Tools<A> {
    async fn call(&self, name: &str, args: Value, connection: &VideoConnection) -> Result<Value> {
        match name {
            "video_models" => {
                serde_json::from_value::<NoArgs>(args)?;
                let models = self.hub.models().await?.into_values().collect::<Vec<_>>();
                Ok(json!({"models":models}))
            }
            "video_create" => {
                let input: Create =
                    serde_json::from_value(args).context("Invalid video_create arguments")?;
                self.hub
                    .create(input, connection, self.access.token(connection))
                    .await
            }
            "video_result" => {
                let input: GetResult =
                    serde_json::from_value(args).context("Invalid video_result arguments")?;
                self.hub
                    .result(input, connection, self.access.token(connection))
                    .await
            }
            _ => bail!("Unknown video tool"),
        }
    }
}

impl<A: VideoAccess> ServerHandler for Tools<A> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("CoreInfra Wizard Video", env!("CARGO_PKG_VERSION")),
        )
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> {
        core::future::ready(Ok(ListToolsResult::with_all_items(vec![
            Tool::new("video_models", "List current Hub video model IDs, providers and price ranges in RUB per second.", schema_for_type::<NoArgs>())
                .with_annotations(ToolAnnotations::new().read_only(true)),
            Tool::new("video_create", "Submit one paid video generation. Keep the returned job to poll or resume. Never retry a submission with an uncertain outcome automatically.", schema_for_type::<Create>())
                .with_annotations(ToolAnnotations::new().read_only(false).idempotent(false)),
            Tool::new("video_result", "Poll an existing job using its original connection. A completed download returns an absolute MP4 path.", schema_for_type::<GetResult>()),
        ])))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let connection = ctx
            .extensions
            .get::<hyper::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<VideoConnection>())
            .ok_or_else(|| ErrorData::internal_error("Missing video grant", None))?;
        let args = Value::Object(request.arguments.unwrap_or_default());
        let response = match self.call(&request.name, args, connection).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
        };
        Ok(response.into())
    }
}

fn empty(status: StatusCode) -> Response<BoxBody<Bytes, Infallible>> {
    let mut response = Response::new(Empty::<Bytes>::new().boxed());
    *response.status_mut() = status;
    response
}

async fn handle<A: VideoAccess>(
    mut request: Request<Incoming>,
    service: StreamableHttpService<Tools<A>, NeverSessionManager>,
    access: Arc<A>,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    if request.uri().path() != "/mcp" || request.uri().query().is_some() {
        return Ok(empty(StatusCode::NOT_FOUND));
    }
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token);
    let connection = access
        .connection()
        .filter(|c| !c.local_bearer.is_empty() && bearer == Some(c.local_bearer.as_str()));
    let Some(connection) = connection else {
        let mut response = empty(StatusCode::UNAUTHORIZED);
        response.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Bearer"),
        );
        return Ok(response);
    };
    request.headers_mut().remove(AUTHORIZATION);
    request.extensions_mut().insert(connection);
    Ok(service.handle(request).await)
}

pub async fn serve(listener: TcpListener, hub: VideoHub, access: impl VideoAccess) -> Result<()> {
    let addr = listener.local_addr()?;
    ensure!(addr.ip().is_loopback(), "Video MCP must bind to loopback");
    let access = Arc::new(access);
    let factory_access = Arc::clone(&access);
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts([addr.to_string()])
        .enforce_origin_validation()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(32 * 1024 * 1024);
    let service = StreamableHttpService::new(
        move || {
            Ok(Tools {
                hub: hub.clone(),
                access: Arc::clone(&factory_access),
            })
        },
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            result = listener.accept() => {
                let (socket, _) = match result {
                    Ok(connection) => connection,
                    Err(error) => {
                        log::warn!("video listener accept failed: {error}");
                        tokio::time::sleep(core::time::Duration::from_millis(500)).await;
                        continue;
                    }
                };
                let service = service.clone();
                let access = Arc::clone(&access);
                clients.spawn(async move {
                    let handler = service_fn(move |req| handle(req, service.clone(), Arc::clone(&access)));
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(socket), handler).await;
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}
