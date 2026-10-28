use crate::{
    destination::{Route, route},
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
use hyper::http::uri::Authority;
use hyper::service::service_fn;
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::io;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[cfg(test)]
#[path = "http_test.rs"]
mod tests;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// After one side of a tunnel closes cleanly, the other may finish for this long. A client
/// that half-closes and then vanishes would otherwise leave a silent remote open forever.
const HALF_CLOSE_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 30 });

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

pub(crate) async fn serve_client(stream: TcpStream, upstream: Arc<Upstream>) {
    let (driver_sender, mut drivers) = mpsc::channel::<Driver>(1);
    let (tunnel_sender, mut tunnels) = mpsc::channel::<Tunnel>(1);
    let service = service_fn(move |request| {
        let upstream = Arc::clone(&upstream);
        let driver_sender = driver_sender.clone();
        let tunnel_sender = tunnel_sender.clone();
        async move { Ok::<_, Infallible>(handle(request, upstream, driver_sender, tunnel_sender).await) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .keep_alive(false)
        // Enables hyper's default 30 s timeout for clients that never send a request head.
        .timer(TokioTimer::new());
    let connection = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::pin!(connection);

    // Drive both sides in this one task. Cancelling it drops both sockets, with no
    // detached upstream tasks. Each client connection serves one request or tunnel.
    // A direct CONNECT has no HTTP client driver; it hands us a TCP socket
    // when the server connection upgrades.
    let driver = tokio::select! {
        _ = &mut connection => None,
        driver = drivers.recv() => driver,
    };
    if let Some(mut driver) = driver {
        tokio::select! {
            _ = &mut connection => {},
            _ = &mut driver => { let _ = (&mut connection).await; },
        }
    }

    if let Ok(tunnel) = tunnels.try_recv()
        && let Ok(client) = tunnel.client.await
    {
        let client = TokioIo::new(client);
        match tunnel.remote {
            Remote::Direct(remote) => relay(client, remote).await,
            Remote::Upstream(remote) => relay(client, TokioIo::new(remote)).await,
        }
    }
}

/// Copy both directions until both close; an error, or [`HALF_CLOSE_TIMEOUT`] after
/// the first clean close, drops both sides. TLS and application protocols inside
/// CONNECT are opaque bytes.
async fn relay(client: impl AsyncRead + AsyncWrite, remote: impl AsyncRead + AsyncWrite) {
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut remote_read, mut remote_write) = tokio::io::split(remote);
    let upload = async {
        tokio::io::copy(&mut client_read, &mut remote_write).await?;
        remote_write.shutdown().await
    };
    let download = async {
        tokio::io::copy(&mut remote_read, &mut client_write).await?;
        client_write.shutdown().await
    };
    tokio::pin!(upload, download);
    tokio::select! {
        result = &mut upload => if result.is_ok() {
            let _ = timeout(HALF_CLOSE_TIMEOUT, download).await;
        },
        result = &mut download => if result.is_ok() {
            let _ = timeout(HALF_CLOSE_TIMEOUT, upload).await;
        },
    }
}

async fn handle(
    mut request: Request<Incoming>,
    upstream: Arc<Upstream>,
    driver: mpsc::Sender<Driver>,
    tunnels: mpsc::Sender<Tunnel>,
) -> Response<Body> {
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
            Err(_) => return error_response(StatusCode::GATEWAY_TIMEOUT),
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
        Ok(Err(_)) => return error_response(StatusCode::BAD_GATEWAY),
        Err(_) => return error_response(StatusCode::GATEWAY_TIMEOUT),
    };
    // Do not impose the connection deadline on streaming request/response bodies.
    let response = sender
        .send_request(request.map(BodyExt::boxed_unsync))
        .await;
    let Ok(mut response) = response else {
        return error_response(StatusCode::BAD_GATEWAY);
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
    crate::connect((host, authority.port_u16().unwrap_or(default_port))).await
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
                .map_err(|_| StatusCode::BAD_GATEWAY)
        }
        Route::Direct => {
            let authority = target.authority().ok_or(StatusCode::BAD_REQUEST)?;
            let port = authority.port_u16().ok_or(StatusCode::BAD_REQUEST)?;
            connect_direct(authority, port)
                .await
                .map(Remote::Direct)
                .map_err(|_| StatusCode::BAD_GATEWAY)
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
