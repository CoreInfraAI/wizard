//! Local HTTP forward proxy. Listed domains never connect directly: allowed ports
//! require CONNECT through the authenticated `CoreInfra` upstream (other methods
//! return 405 with Allow: CONNECT); unlisted ports return 403 for every method.
//! Unmatched domains and explicit IP addresses connect directly, without proxy credentials.
//! Malformed destinations are rejected before opening a connection.
//! Failed accepts, e.g. while file descriptors are exhausted, are retried: the listener
//! stops only on request. Outgoing sockets use TCP keepalive, so a peer that vanishes
//! without closing does not hold a tunnel open forever; neither does a half-closed tunnel.
//! Dropping the handle requests shutdown; use [`Proxy::stop`] to wait for all sockets to close.

extern crate alloc;

use alloc::sync::Arc;
use core::{net::SocketAddr, time::Duration};
use socket2::{SockRef, TcpKeepalive};
use std::env::consts::{ARCH, OS};
use std::io;
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::{oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

mod destination;
mod http;
mod upstream;

pub use upstream::{HOST as UPSTREAM_HOST, PORT as UPSTREAM_PORT, USERNAME as UPSTREAM_USERNAME};

/// Listener of Claude Desktop, with its own token.
pub const CLAUDE_DESKTOP_ADDRESS: &str = "127.0.0.1:33787";
/// Listener of `ChatGPT`, with its own token.
pub const CHATGPT_ADDRESS: &str = "127.0.0.1:33788";

/// While descriptors are exhausted, an immediate retry would fail again at once.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);
/// Accept failures are logged at most this often, so they cannot rotate older entries out of the log.
const ACCEPT_WARNING_INTERVAL: Duration = Duration::from_secs(60);

/// Detects a dead peer within about three minutes. Tunnels have no idle timeout,
/// because idle WebSocket and streaming connections are legitimate.
const KEEPALIVE: TcpKeepalive = TcpKeepalive::new()
    .with_time(Duration::from_secs(60))
    .with_interval(Duration::from_secs(15));

/// `User-Agent` of requests Wizard itself sends, e.g. `CoreInfraAI-Wizard/<version> (macos; aarch64)`.
/// OS and architecture are those of the running binary.
#[must_use]
pub fn user_agent() -> String {
    format!(
        "CoreInfraAI-Wizard/{} ({OS}; {ARCH})",
        env!("CARGO_PKG_VERSION")
    )
}

/// Owns a running listener. No settings are read or watched by this crate.
pub struct Proxy {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
    stopped: watch::Receiver<()>,
}

impl Proxy {
    /// Wait for the listener task to finish, including panic or cancellation.
    /// The returned future does not borrow the handle and can be observed independently.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static + use<> {
        let mut stopped = self.stopped.clone();
        async move {
            let _ = stopped.changed().await;
        }
    }

    /// Stop accepting requests, cancel active connections, and wait for their cleanup.
    pub async fn stop(self) -> io::Result<()> {
        drop(self.shutdown);
        self.task.await.map_err(io::Error::other)
    }
}

/// Bind `address` and start proxying with a snapshot of `token`.
/// Upstream connectivity is checked per request, not during startup.
pub async fn start(address: &str, token: &str) -> io::Result<Proxy> {
    if token.is_empty() {
        // TODO: Decide the final behavior when the saved CoreInfra token is absent.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CoreInfra token is not set",
        ));
    }

    let authorization = upstream::authorization(token)?;
    let tls = tokio::task::spawn_blocking(upstream::tls_config)
        .await
        .map_err(io::Error::other)??;
    let upstream = Arc::new(upstream::Upstream::new(tls, authorization));
    let listener = TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let (shutdown, receiver) = oneshot::channel();
    let (finished, stopped) = watch::channel(());
    let task = tokio::spawn(async move {
        // Dropping the only sender wakes observers on every task-exit path.
        let _finished = finished;
        serve(listener, address, upstream, receiver).await;
    });
    Ok(Proxy {
        shutdown,
        task,
        stopped,
    })
}

async fn serve(
    listener: TcpListener,
    address: SocketAddr,
    upstream: Arc<upstream::Upstream>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut connections = JoinSet::new();
    let mut warned_at: Option<Instant> = None;
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let upstream = Arc::clone(&upstream);
                        connections.spawn(http::serve_client(stream, upstream));
                    }
                    // Only this client is gone; the listener is unaffected.
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                        ) => {}
                    // Typically EMFILE: closing tunnels will free descriptors.
                    Err(error) => {
                        if warned_at.is_none_or(|at| at.elapsed() >= ACCEPT_WARNING_INTERVAL) {
                            warned_at = Some(Instant::now());
                            log::warn!(
                                "local proxy on {address} cannot accept connections, retrying: {error}"
                            );
                        }
                        tokio::select! {
                            biased;
                            _ = &mut shutdown => break,
                            () = tokio::time::sleep(ACCEPT_RETRY_DELAY) => {},
                        }
                    }
                }
            }
        }
    }

    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

/// Connect to a remote host, enabling [`KEEPALIVE`] on the socket.
async fn connect(address: impl ToSocketAddrs) -> io::Result<TcpStream> {
    let stream = TcpStream::connect(address).await?;
    SockRef::from(&stream).set_tcp_keepalive(&KEEPALIVE)?;
    Ok(stream)
}
