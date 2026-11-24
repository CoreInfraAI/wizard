use super::{Create, GetResult, MediaHub, MediaKey};
use alloc::sync::Arc;
use anyhow::{Context as _, Result, bail, ensure};
use core::{convert::Infallible, future::Future, net::SocketAddr};
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
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};

struct Tools {
    hub: MediaHub,
    key: Option<Arc<MediaKey>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

impl Tools {
    async fn call(&self, name: &str, args: Value) -> Result<Value> {
        match name {
            "video_models" => {
                serde_json::from_value::<NoArgs>(args)?;
                let models = self.hub.models().await?.into_values().collect::<Vec<_>>();
                Ok(json!({"models":models}))
            }
            "video_create" => {
                let input: Create =
                    serde_json::from_value(args).context("Invalid video_create arguments")?;
                self.hub.create(input, self.key.as_deref()).await
            }
            "video_result" => {
                let input: GetResult =
                    serde_json::from_value(args).context("Invalid video_result arguments")?;
                self.hub.result(input, self.key.as_deref()).await
            }
            _ => bail!("Unknown media tool"),
        }
    }
}

pub(super) fn server_info() -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
        Implementation::new("CoreInfra Wizard Media", env!("CARGO_PKG_VERSION")),
    )
}

impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        server_info()
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
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Value::Object(request.arguments.unwrap_or_default());
        let response = match self.call(&request.name, args).await {
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

type Routes = Arc<Vec<(String, StreamableHttpService<Tools, NeverSessionManager>)>>;

async fn handle(
    mut request: Request<Incoming>,
    routes: Routes,
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
    let Some((_, service)) = routes
        .iter()
        .find(|(grant, _)| Some(grant.as_str()) == bearer)
    else {
        let mut response = empty(StatusCode::UNAUTHORIZED);
        response.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Bearer"),
        );
        return Ok(response);
    };
    request.headers_mut().remove(AUTHORIZATION);
    Ok(service.handle(request).await)
}

#[must_use]
pub fn new_bearer() -> String {
    format!("wizard-media-{}", uuid::Uuid::new_v4())
}

/// Publish the new listener's grant before accepting requests.
pub async fn start(
    listener: TcpListener,
    hub: MediaHub,
    key: Option<MediaKey>,
    register: impl FnOnce(&str) -> Result<()> + Send + 'static,
) -> Result<JoinHandle<()>> {
    let addr = listener.local_addr()?;
    ensure!(addr.ip().is_loopback(), "Media MCP must bind to loopback");
    let bearer = tokio::task::spawn_blocking(move || {
        let bearer = new_bearer();
        register(&bearer)?;
        anyhow::Ok(bearer)
    })
    .await
    .context("Media installation task failed")??;
    Ok(tokio::spawn(serve(
        listener,
        addr,
        hub,
        vec![(bearer, key)],
    )))
}

pub async fn serve(
    listener: TcpListener,
    addr: SocketAddr,
    hub: MediaHub,
    grants: Vec<(String, Option<MediaKey>)>,
) {
    let mut routes = Vec::new();
    for (grant, key) in grants {
        let key = key.map(Arc::new);
        let hub = hub.clone();
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([addr.to_string()])
            .enforce_origin_validation()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_max_request_body_bytes(super::MAX_REQUEST_BYTES);
        let service = StreamableHttpService::new(
            move || {
                Ok(Tools {
                    hub: hub.clone(),
                    key: key.clone(),
                })
            },
            Arc::new(NeverSessionManager::default()),
            config,
        );
        routes.push((grant, service));
    }
    let routes = Arc::new(routes);
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            result = listener.accept() => {
                let (socket, _) = match result {
                    Ok(connection) => connection,
                    Err(error) => {
                        log::warn!("media listener accept failed: {error}");
                        tokio::time::sleep(core::time::Duration::from_millis(500)).await;
                        continue;
                    }
                };
                let routes = Arc::clone(&routes);
                clients.spawn(async move {
                    let handler = service_fn(move |req| handle(req, Arc::clone(&routes)));
                    let _ = http1::Builder::new().serve_connection(TokioIo::new(socket), handler).await;
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}
