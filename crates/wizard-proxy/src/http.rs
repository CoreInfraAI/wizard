use crate::{
    ProxySlot, ServerState,
    control::{Command, TIMEOUT},
    destination::{Route, route},
    diagnostics, run_proxy_session,
    upstream::Upstream,
};
use alloc::sync::Arc;
use core::{convert::Infallible, future::Future, pin::Pin, time::Duration};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    ALLOW, CONNECTION, CONTENT_LENGTH, HOST, HeaderName, HeaderValue, PROXY_AUTHORIZATION,
    TRANSFER_ENCODING, USER_AGENT,
};
use hyper::http::uri::{Authority, PathAndQuery};
use hyper::service::service_fn;
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

#[cfg(test)]
#[path = "http_test.rs"]
mod tests;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

type Body = UnsyncBoxBody<Bytes, hyper::Error>;
type Driver = Pin<Box<dyn Future<Output = Result<(), hyper::Error>> + Send>>;

enum Remote {
    Direct(TcpStream),
    Upstream(Upgraded),
}

struct Tunnel {
    client: OnUpgrade,
    remote: Remote,
}

// The HTTP service announces the request kind before starting any proxy I/O.
// The connection task then supplies an upstream or a management response.
enum Dispatch {
    Proxy(oneshot::Sender<Option<Arc<Upstream>>>),
    Management(Command, oneshot::Sender<Response<Body>>),
}

pub(crate) async fn serve_client(
    stream: TcpStream,
    slot: Arc<ProxySlot>,
    management: Option<Arc<ServerState>>,
) {
    let managed = management.is_some();
    let address = match stream.local_addr() {
        Ok(address) => Some(address.to_string()),
        Err(error) => {
            diagnostics::error("proxy local socket address lookup", &error);
            None
        }
    };
    let (dispatch, mut requests) = mpsc::channel(1);
    let (driver_sender, mut drivers) = mpsc::channel::<Driver>(1);
    let (tunnel_sender, mut tunnels) = mpsc::channel::<Tunnel>(1);
    let service = service_fn(move |request: Request<Incoming>| {
        let dispatch = dispatch.clone();
        let address = address.clone();
        let driver_sender = driver_sender.clone();
        let tunnel_sender = tunnel_sender.clone();
        async move {
            let response = if managed
                && request.method() != Method::CONNECT
                && request.uri().scheme().is_none()
                && request.uri().authority().is_none()
            {
                match management_command(&request, address.as_deref()) {
                    Err((status, allow)) => {
                        let mut response = error_response(status);
                        if let Some(allow) = allow {
                            response
                                .headers_mut()
                                .insert(ALLOW, HeaderValue::from_static(allow));
                        }
                        response
                    }
                    Ok(command) => {
                        let (reply, response) = oneshot::channel();
                        if dispatch
                            .send(Dispatch::Management(command, reply))
                            .await
                            .is_err()
                        {
                            return Ok::<_, Infallible>(error_response(
                                StatusCode::SERVICE_UNAVAILABLE,
                            ));
                        }
                        response
                            .await
                            .unwrap_or_else(|_| error_response(StatusCode::SERVICE_UNAVAILABLE))
                    }
                }
            } else {
                let (ready, upstream) = oneshot::channel();
                if dispatch.send(Dispatch::Proxy(ready)).await.is_err() {
                    return Ok::<_, Infallible>(error_response(StatusCode::SERVICE_UNAVAILABLE));
                }
                handle(
                    request,
                    upstream.await.unwrap_or(None),
                    driver_sender,
                    tunnel_sender,
                )
                .await
            };
            Ok::<_, Infallible>(response)
        }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.keep_alive(false);
    if managed {
        builder
            .timer(TokioTimer::new())
            .header_read_timeout(TIMEOUT);
    }
    let connection = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    // Own the complete connection future, not just its pinned reference, so the
    // generation wrapper can drop every socket before releasing its read guard.
    let mut session = Box::pin(async move {
        tokio::pin!(connection);
        let driver = tokio::select! {
            result = &mut connection => {
                if let Err(error) = result {
                    diagnostics::error("client HTTP connection", &error);
                }
                None
            },
            driver = drivers.recv() => driver,
        };
        if let Some(mut driver) = driver {
            tokio::select! {
                result = &mut connection => {
                    if let Err(error) = result {
                        diagnostics::error("client HTTP connection", &error);
                    }
                },
                result = &mut driver => {
                    if let Err(error) = result {
                        diagnostics::error("remote HTTP connection", &error);
                    }
                    if let Err(error) = (&mut connection).await {
                        diagnostics::error("client HTTP connection", &error);
                    }
                },
            }
        }
        if let Ok(tunnel) = tunnels.try_recv() {
            let client = match tunnel.client.await {
                Ok(client) => client,
                Err(error) => {
                    diagnostics::error("client CONNECT upgrade", &error);
                    return;
                }
            };
            let mut client = TokioIo::new(client);
            let result = match tunnel.remote {
                Remote::Direct(mut remote) => copy_bidirectional(&mut client, &mut remote).await,
                Remote::Upstream(remote) => {
                    copy_bidirectional(&mut client, &mut TokioIo::new(remote)).await
                }
            };
            if let Err(error) = result {
                diagnostics::error("proxy tunnel transfer", &error);
            }
        }
    });
    let request = tokio::select! {
        request = requests.recv() => request,
        () = &mut session => return,
    };
    match request {
        Some(Dispatch::Proxy(ready)) => {
            let generation = slot.current.read().await.clone();
            if let Some(generation) = generation {
                let upstream = Arc::clone(&generation.upstream);
                run_proxy_session(generation, async move {
                    if ready.send(Some(upstream)).is_err() {
                        log::error!("proxy dispatch receiver closed before generation delivery");
                    }
                    session.await;
                })
                .await;
            } else {
                if ready.send(None).is_err() {
                    log::error!("proxy dispatch receiver closed before disabled response");
                }
                session.await;
            }
        }
        Some(Dispatch::Management(command, reply)) => {
            let Some(state) = management else { return };
            let _active = state.management.read().await;
            // Ownership in this inner scope ensures socket cleanup precedes the
            // management guard's release, including on task cancellation.
            {
                let session = session;
                let stopping = *state.stopping.borrow();
                let response = if stopping {
                    error_response(StatusCode::SERVICE_UNAVAILABLE)
                } else {
                    match command {
                        Command::Status => json_response(&state.status_with_lock().await),
                        Command::Reload => json_response(&state.reload_with_lock().await),
                        Command::Shutdown => json_response(&state.shutdown_with_lock().await),
                    }
                };
                // Do not race accepted work against client disconnects. Only the
                // reply attempt is bounded; shutdown waits on the active guard.
                if reply.send(response).is_err() {
                    log::error!("management response receiver closed");
                }
                if timeout(TIMEOUT, session).await.is_err() {
                    log::error!("management reply timed out");
                }
            }
        }
        None => {}
    }
}

fn management_command(
    request: &Request<Incoming>,
    address: Option<&str>,
) -> Result<Command, (StatusCode, Option<&'static str>)> {
    if request
        .headers()
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        != address
        || address.is_none()
        || request.headers().contains_key(TRANSFER_ENCODING)
        || request
            .headers()
            .get(CONTENT_LENGTH)
            .is_some_and(|length| length != "0")
    {
        return Err((StatusCode::BAD_REQUEST, None));
    }
    let (method, command) = match request.uri().path_and_query().map(PathAndQuery::as_str) {
        Some("/status") => (Method::GET, Command::Status),
        Some("/reload") => (Method::POST, Command::Reload),
        Some("/shutdown") => (Method::POST, Command::Shutdown),
        _ => return Err((StatusCode::NOT_FOUND, None)),
    };
    if request.method() != method {
        let allow = match command {
            Command::Status => "GET",
            Command::Reload | Command::Shutdown => "POST",
        };
        return Err((StatusCode::METHOD_NOT_ALLOWED, Some(allow)));
    }
    Ok(command)
}

fn json_response(value: &impl serde::Serialize) -> Response<Body> {
    let Ok(bytes) = serde_json::to_vec(value) else {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut response = Response::new(
        Full::new(Bytes::from(bytes))
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
        .headers_mut()
        .insert("content-type", HeaderValue::from_static("application/json"));
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn handle(
    mut request: Request<Incoming>,
    upstream: Option<Arc<Upstream>>,
    driver: mpsc::Sender<Driver>,
    tunnels: mpsc::Sender<Tunnel>,
) -> Response<Body> {
    let Some(upstream) = upstream else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE);
    };

    if request.method() == Method::CONNECT {
        if !valid_connect_target(request.uri())
            || request.headers().contains_key(TRANSFER_ENCODING)
            || request
                .headers()
                .get(CONTENT_LENGTH)
                .is_some_and(|length| length != "0")
        {
            return error_response(StatusCode::BAD_REQUEST);
        }

        let Ok(route) = route(&request.uri().to_string()) else {
            return error_response(StatusCode::BAD_REQUEST);
        };
        let result = timeout(
            CONNECT_TIMEOUT,
            connect_remote(request.uri().clone(), route, &upstream, driver),
        )
        .await;
        let remote = match result {
            Ok(Ok(remote)) => remote,
            Ok(Err(status)) => return error_response(status),
            Err(_) => {
                log::error!("CONNECT establishment timed out");
                return error_response(StatusCode::GATEWAY_TIMEOUT);
            }
        };
        let tunnel = Tunnel {
            client: hyper::upgrade::on(&mut request),
            remote,
        };
        if tunnels.send(tunnel).await.is_err() {
            return error_response(StatusCode::BAD_GATEWAY);
        }
        return Response::new(empty_body());
    }

    let Some(authority) = request.uri().authority().cloned() else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    if request.uri().scheme_str() != Some("http")
        || authority.host().is_empty()
        || authority.as_str().contains('@')
        || authority.port_u16() == Some(0)
    {
        return error_response(StatusCode::BAD_REQUEST);
    }
    // Only an absent port defaults to 80; malformed explicit ports must fail.
    let destination = if authority.as_str() == authority.host() {
        format!("{authority}:80")
    } else {
        authority.to_string()
    };
    match route(&destination) {
        Err(_) => return error_response(StatusCode::BAD_REQUEST),
        Ok(Route::Block) => return error_response(StatusCode::FORBIDDEN),
        Ok(Route::Proxy) => {
            // Hub supports CONNECT, not absolute-form HTTP forwarding.
            let mut response = error_response(StatusCode::METHOD_NOT_ALLOWED);
            response
                .headers_mut()
                .insert(ALLOW, HeaderValue::from_static("CONNECT"));
            return response;
        }
        Ok(Route::Direct) => {}
    }

    let Ok(host) = HeaderValue::from_str(authority.as_str()) else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    if strip_hop_headers(request.headers_mut()).is_err() {
        return error_response(StatusCode::BAD_REQUEST);
    }
    request.headers_mut().insert(HOST, host);

    // Origin servers receive origin-form, not the proxy's absolute-form URI.
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |path| path.as_str());
    let Ok(uri) = Uri::builder().path_and_query(path).build() else {
        return error_response(StatusCode::BAD_REQUEST);
    };
    *request.uri_mut() = uri;
    request
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));

    let connection = async {
        let stream = connect_direct(&authority, 80).await?;
        open_connection(stream, driver).await
    };
    let mut sender = match timeout(CONNECT_TIMEOUT, connection).await {
        Ok(Ok(sender)) => sender,
        Ok(Err(error)) => {
            diagnostics::error("direct HTTP connection establishment", &error);
            return error_response(StatusCode::BAD_GATEWAY);
        }
        Err(_) => {
            log::error!("direct HTTP connection establishment timed out");
            return error_response(StatusCode::GATEWAY_TIMEOUT);
        }
    };
    // Do not impose the connection deadline on streaming request/response bodies.
    let response = sender
        .send_request(request.map(BodyExt::boxed_unsync))
        .await;
    let mut response = match response {
        Ok(response) => response,
        Err(error) => {
            diagnostics::error("direct HTTP request", &error);
            return error_response(StatusCode::BAD_GATEWAY);
        }
    };
    if response.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED
        || response.status() == StatusCode::SWITCHING_PROTOCOLS
        || strip_hop_headers(response.headers_mut()).is_err()
    {
        return error_response(StatusCode::BAD_GATEWAY);
    }
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response.map(BodyExt::boxed_unsync)
}

fn valid_connect_target(uri: &Uri) -> bool {
    uri.scheme().is_none()
        && uri.path_and_query().is_none()
        && uri.authority().is_some_and(|authority| {
            !authority.host().is_empty()
                && !authority.as_str().contains('@')
                && authority.port_u16().is_some_and(|port| port != 0)
        })
}

async fn connect_direct(authority: &Authority, default_port: u16) -> io::Result<TcpStream> {
    let host = authority.host();
    // HTTP authorities retain IPv6 brackets; the socket resolver needs the bare IP.
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    TcpStream::connect((host, authority.port_u16().unwrap_or(default_port))).await
}

async fn connect_remote(
    target: Uri,
    route: Route,
    upstream: &Upstream,
    driver: mpsc::Sender<Driver>,
) -> Result<Remote, StatusCode> {
    match route {
        Route::Block => Err(StatusCode::FORBIDDEN),
        Route::Proxy => {
            // An upstream failure must never fall back to a direct connection.
            connect_tunnel(target, upstream, driver)
                .await
                .map(Remote::Upstream)
                .map_err(|error| {
                    diagnostics::error("upstream CONNECT establishment", &error);
                    StatusCode::BAD_GATEWAY
                })
        }
        Route::Direct => {
            let authority = target.authority().ok_or(StatusCode::BAD_REQUEST)?;
            let port = authority.port_u16().ok_or(StatusCode::BAD_REQUEST)?;
            connect_direct(authority, port)
                .await
                .map(Remote::Direct)
                .map_err(|error| {
                    diagnostics::error("direct CONNECT establishment", &error);
                    StatusCode::BAD_GATEWAY
                })
        }
    }
}

async fn open_upstream(
    upstream: &Upstream,
    driver: mpsc::Sender<Driver>,
) -> io::Result<hyper::client::conn::http1::SendRequest<Body>> {
    let stream = upstream.connect().await?;
    open_connection(stream, driver).await
}

async fn open_connection(
    stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
    driver: mpsc::Sender<Driver>,
) -> io::Result<hyper::client::conn::http1::SendRequest<Body>> {
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    driver
        .send(Box::pin(connection.with_upgrades()))
        .await
        .map_err(|_| io::Error::other("client connection closed"))?;
    Ok(sender)
}

async fn connect_tunnel(
    target: Uri,
    upstream: &Upstream,
    driver: mpsc::Sender<Driver>,
) -> io::Result<Upgraded> {
    let host = target
        .authority()
        .ok_or_else(|| io::Error::other("CONNECT authority missing"))?
        .as_str();
    let request = Request::builder()
        .method(Method::CONNECT)
        .uri(target.clone())
        .header(HOST, host)
        .header(PROXY_AUTHORIZATION, upstream.authorization.clone())
        .header(USER_AGENT, crate::user_agent())
        .body(empty_body())
        .map_err(io::Error::other)?;
    let mut sender = open_upstream(upstream, driver).await?;
    let mut response = sender
        .send_request(request)
        .await
        .map_err(io::Error::other)?;
    if !response.status().is_success() {
        log::error!(
            "upstream rejected CONNECT: status={}",
            response.status().as_u16()
        );
        return Err(io::Error::other("upstream rejected CONNECT"));
    }
    hyper::upgrade::on(&mut response)
        .await
        .map_err(io::Error::other)
}

fn strip_hop_headers(headers: &mut HeaderMap) -> Result<(), hyper::header::InvalidHeaderName> {
    let nominated = headers
        .get_all(CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .map(<[u8]>::trim_ascii)
        .filter(|name| !name.is_empty())
        .map(HeaderName::from_bytes)
        .collect::<Result<Vec<_>, _>>()?;
    for name in nominated {
        headers.remove(name);
    }
    if headers.contains_key(TRANSFER_ENCODING) {
        headers.remove(CONTENT_LENGTH);
    }
    for name in [
        "connection",
        "proxy-connection",
        "keep-alive",
        "te",
        "transfer-encoding",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
    ] {
        headers.remove(name);
    }
    Ok(())
}

fn empty_body() -> Body {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn error_response(status: StatusCode) -> Response<Body> {
    log::warn!("proxy request rejected: status={}", status.as_u16());
    // Never expose upstream response bodies, credentials, or request details in errors.
    let message = status.canonical_reason().unwrap_or("Proxy error");
    let mut response = Response::new(
        Full::new(Bytes::from_static(message.as_bytes()))
            .map_err(|never| match never {})
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONNECTION, HeaderValue::from_static("close"));
    response
}
