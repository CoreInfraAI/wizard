//! Local HTTP forward proxy. All traffic goes through the authenticated `CoreInfra` upstream.
//! Dropping the handle requests shutdown; use [`Proxy::stop`] to wait for all sockets to close.

extern crate alloc;

use alloc::sync::Arc;
use std::io;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

mod http;
mod upstream;

pub const LISTEN_ADDRESS: &str = "127.0.0.1:33787";

/// Owns a running listener. No settings are read or watched by this crate.
pub struct Proxy {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<io::Result<()>>,
    stopped: watch::Receiver<()>,
}

impl Proxy {
    /// Wait for the listener task to finish, including failure, panic or cancellation.
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
        self.task.await.map_err(io::Error::other)?
    }
}

/// Bind the fixed loopback address and start proxying with a snapshot of `token`.
/// Upstream connectivity is checked per request, not during startup.
pub async fn start(token: &str) -> io::Result<Proxy> {
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
    let listener = TcpListener::bind(LISTEN_ADDRESS).await?;
    let (shutdown, receiver) = oneshot::channel();
    let (finished, stopped) = watch::channel(());
    let task = tokio::spawn(async move {
        // Dropping the only sender wakes observers on every task-exit path.
        let _finished = finished;
        serve(listener, upstream, receiver).await
    });
    Ok(Proxy {
        shutdown,
        task,
        stopped,
    })
}

async fn serve(
    listener: TcpListener,
    upstream: Arc<upstream::Upstream>,
    mut shutdown: oneshot::Receiver<()>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            Some(_) = connections.join_next(), if !connections.is_empty() => {},
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let upstream = Arc::clone(&upstream);
                        connections.spawn(http::serve_client(stream, upstream));
                    }
                    Err(error) => break Err(error),
                }
            }
        }
    };

    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    result
}
