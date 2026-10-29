//! Local HTTP forward proxy. Listed domains never connect directly: allowed ports
//! require CONNECT through the authenticated `CoreInfra` upstream (other methods
//! return 405 with Allow: CONNECT); unlisted ports return 403 for every method.
//! Unmatched domains and explicit IP addresses connect directly, without proxy credentials.
//! Malformed destinations are rejected before opening a connection.
//! Dropping the handle requests shutdown; use [`Proxy::stop`] to wait for all sockets to close.

extern crate alloc;

use alloc::sync::Arc;
use std::env::consts::{ARCH, OS};
use std::io;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, RwLock, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::rustls::ClientConfig;

pub mod control;
pub mod diagnostics;

mod destination;
mod http;
mod upstream;

#[cfg(test)]
#[path = "server_test.rs"]
mod server_tests;

pub use upstream::{HOST as UPSTREAM_HOST, PORT as UPSTREAM_PORT, USERNAME as UPSTREAM_USERNAME};

/// Listener of Claude Desktop, with its own token.
pub const CLAUDE_DESKTOP_ADDRESS: &str = "127.0.0.1:33787";
/// Listener of `ChatGPT`, with its own token.
pub const CHATGPT_ADDRESS: &str = "127.0.0.1:33788";

/// `User-Agent` of requests Wizard itself sends, e.g. `CoreInfraAI-Wizard/<version> (macos; aarch64)`.
/// OS and architecture are those of the running binary.
#[must_use]
pub fn user_agent() -> String {
    format!(
        "CoreInfraAI-Wizard/{} ({OS}; {ARCH})",
        env!("CARGO_PKG_VERSION")
    )
}

/// Owns a standalone listener. This handle does not read or watch settings.
pub struct Proxy {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<io::Result<()>>,
    stopped: watch::Receiver<()>,
    slot: Arc<ProxySlot>,
    tls: ClientConfig,
}

// Shared configuration, never listener handles: tasks must not own themselves
// through this state. No Debug because the slots contain credentials.
struct ServerState {
    reload_lock: Mutex<()>,
    tls: ClientConfig,
    claude_desktop: Arc<ProxySlot>,
    chatgpt: Arc<ProxySlot>,
    // Shutdown waits for accepted commands and their bounded reply attempts.
    management: RwLock<()>,
    stopping: watch::Sender<bool>,
}

impl ServerState {
    fn new(tls: ClientConfig) -> Self {
        Self {
            reload_lock: Mutex::new(()),
            tls,
            claude_desktop: Arc::new(ProxySlot::default()),
            chatgpt: Arc::new(ProxySlot::default()),
            management: RwLock::new(()),
            stopping: watch::channel(false).0,
        }
    }

    async fn status_with_lock(&self) -> control::Status {
        let _guard = self.reload_lock.lock().await;
        self.current_status().await
    }

    // Called with reload_lock held, so the pair cannot be observed mid-update.
    async fn current_status(&self) -> control::Status {
        control::Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            claude_desktop_enabled: self.claude_desktop.current.read().await.is_some(),
            chatgpt_enabled: self.chatgpt.current.read().await.is_some(),
        }
    }

    async fn shutdown_with_lock(&self) -> control::ShutdownResponse {
        let _guard = self.reload_lock.lock().await;
        log::info!("graceful proxy shutdown requested through HTTP");
        self.stopping.send_replace(true);
        control::ShutdownResponse {
            pid: std::process::id(),
        }
    }

    async fn reload_with_lock(&self) -> control::ReloadResponse {
        let _guard = self.reload_lock.lock().await;
        let started = std::time::Instant::now();
        log::info!("proxy settings reload started");
        let result = async {
            if *self.stopping.borrow() {
                return Err("proxy server is stopping");
            }
            let mut settings = wizard_settings::load_from_file()
                .inspect_err(|error| diagnostics::error("proxy settings load", error.as_ref()))
                .map_err(|_| "failed to load Wizard settings")?;
            let claude_token = settings.claude_proxy_enabled.take();
            let chatgpt_token = settings.codex_proxy_enabled.take();
            drop(settings);

            log::info!("applying Claude Desktop proxy settings");
            let claude = self
                .claude_desktop
                .set_token(claude_token.as_deref(), &self.tls)
                .await
                .inspect_err(|error| diagnostics::error("Claude Desktop generation update", error))
                .map_err(|_| "failed to update Claude Desktop proxy");
            log::info!("applying ChatGPT proxy settings");
            let chatgpt = self
                .chatgpt
                .set_token(chatgpt_token.as_deref(), &self.tls)
                .await
                .inspect_err(|error| diagnostics::error("ChatGPT generation update", error))
                .map_err(|_| "failed to update ChatGPT proxy");
            // Attempt both, retaining the first error without rolling back success.
            claude.and(chatgpt)
        }
        .await;
        let status = self.current_status().await;
        match &result {
            Ok(()) => log::info!(
                "proxy settings reload completed: claude_desktop={}, chatgpt={}, elapsed_ms={}",
                status.claude_desktop_enabled,
                status.chatgpt_enabled,
                started.elapsed().as_millis(),
            ),
            // These errors are static diagnostics, never settings or credentials.
            Err(error) => log::error!(
                "proxy settings reload failed: {error}; claude_desktop={}, chatgpt={}, elapsed_ms={}",
                status.claude_desktop_enabled,
                status.chatgpt_enabled,
                started.elapsed().as_millis(),
            ),
        }
        if result.is_ok() && !status.enabled() {
            // Prevent later queued Reloads from re-enabling during shutdown.
            // run_listeners still waits for the current HTTP reply to finish.
            self.stopping.send_replace(true);
        }
        control::ReloadResponse {
            status,
            error: result.err().map(str::to_owned),
        }
    }
}

// No Debug: generations contain tokens and upstream credentials.
#[derive(Default)]
struct ProxySlot {
    current: RwLock<Option<Arc<Generation>>>,
}

struct Generation {
    token: String,
    upstream: Arc<upstream::Upstream>,
    cancelled: watch::Sender<bool>,
    active: RwLock<()>,
}

impl ProxySlot {
    // Writers must be serialized through the whole update, including draining.
    // The standalone handle enforces this with &mut self; the managed server
    // holds ServerState::reload_lock across both slot updates.
    async fn set_token(&self, token: Option<&str>, tls: &ClientConfig) -> io::Result<()> {
        let mut current = self.current.write().await;
        if current.as_ref().map(|generation| generation.token.as_str()) == token {
            return Ok(());
        }

        let next = match token {
            None => None,
            Some(token) => {
                validate_token(token)?;
                let authorization = upstream::authorization(token)?;
                let upstream = upstream::Upstream::new(tls.clone(), authorization);
                let (cancelled, _) = watch::channel(false);
                Some(Arc::new(Generation {
                    token: token.to_owned(),
                    upstream: Arc::new(upstream),
                    cancelled,
                    active: RwLock::new(()),
                }))
            }
        };

        let change = match (current.is_some(), next.is_some()) {
            (false, true) => "enabled",
            (true, true) => "replaced",
            (_, false) => "disabled",
        };
        let previous = core::mem::replace(&mut *current, next);
        log::info!("proxy generation {change}");
        if let Some(previous) = previous {
            let started = std::time::Instant::now();
            log::info!("cancelling and draining previous proxy generation");
            previous.cancelled.send_replace(true);
            drop(current);

            // Old sessions drop their sockets before releasing their read guards.
            // Late arrivals check cancellation before doing any proxy I/O.
            let _drained = previous.active.write().await;
            log::info!(
                "previous proxy generation drained: elapsed_ms={}",
                started.elapsed().as_millis(),
            );
        }
        Ok(())
    }
}

impl Proxy {
    /// Update proxying without rebinding the listener. None rejects proxy requests
    /// with HTTP 503. Unchanged tokens preserve sessions; changes cancel and drain
    /// the old generation. Preparation errors preserve the current generation.
    /// The caller must finish awaiting this operation to wait for session cleanup.
    pub async fn set_token(&mut self, token: Option<&str>) -> io::Result<()> {
        self.slot.set_token(token, &self.tls).await
    }

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

/// Read Wizard settings and run both fixed-address listeners with unauthenticated
/// GET /status, POST /reload and POST /shutdown. With both proxies disabled,
/// return without binding.
/// TLS roots load once for the pair; disabled proxy traffic receives HTTP 503.
/// Accepted management commands and bounded reply attempts finish before shutdown.
/// This function does not register OS services or change saved settings.
pub async fn run_managed(shutdown: impl Future<Output = ()>) -> io::Result<()> {
    // Avoid loading system certificates when no proxy is needed. Reload rereads
    // after the potentially slow TLS initialization, using the latest settings.
    if !wizard_settings::load_from_file()
        .inspect_err(|error| diagnostics::error("proxy startup settings load", error.as_ref()))
        .map_err(|_| io::Error::other("failed to load Wizard settings"))?
        .local_proxy_enabled()
    {
        log::info!("proxy startup skipped: both proxies are disabled");
        return Ok(());
    }
    log::info!("loading proxy TLS roots");
    let tls = tokio::task::spawn_blocking(upstream::tls_config)
        .await
        .map_err(io::Error::other)??;
    let state = Arc::new(ServerState::new(tls));
    let response = state.reload_with_lock().await;
    if let Some(error) = response.error {
        return Err(io::Error::other(error));
    }
    if !response.status.enabled() {
        return Ok(());
    }

    // Bind both before starting tasks. If the second bind fails, the first
    // listener is dropped too; a disabled app's port must also be available.
    let claude = TcpListener::bind(CLAUDE_DESKTOP_ADDRESS)
        .await
        .inspect_err(|error| {
            diagnostics::error("Claude Desktop listener bind", error);
        })?;
    let chatgpt = TcpListener::bind(CHATGPT_ADDRESS)
        .await
        .inspect_err(|error| {
            diagnostics::error("ChatGPT listener bind", error);
        })?;
    log::info!(
        "proxy listeners bound: claude_desktop={CLAUDE_DESKTOP_ADDRESS}, chatgpt={CHATGPT_ADDRESS}"
    );
    run_listeners(state, claude, chatgpt, shutdown).await
}

async fn run_listeners(
    state: Arc<ServerState>,
    claude: TcpListener,
    chatgpt: TcpListener,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    // Handles stay private. Only ServerState::reload changes managed tokens.
    let claude = spawn(
        claude,
        Arc::clone(&state.claude_desktop),
        state.tls.clone(),
        Some(Arc::clone(&state)),
    );
    let chatgpt = spawn(
        chatgpt,
        Arc::clone(&state.chatgpt),
        state.tls.clone(),
        Some(Arc::clone(&state)),
    );
    let mut stopping = state.stopping.subscribe();

    tokio::pin!(shutdown);
    let result = tokio::select! {
        biased;
        () = &mut shutdown => {
            log::info!("proxy shutdown requested by caller");
            Ok(())
        },
        _ = stopping.wait_for(|stopping| *stopping) => {
            log::info!("proxy shutdown requested through management");
            Ok(())
        },
        () = claude.stopped() => {
            log::error!("Claude Desktop listener stopped unexpectedly");
            Err(io::Error::other("Claude Desktop listener stopped unexpectedly"))
        },
        () = chatgpt.stopped() => {
            log::error!("ChatGPT listener stopped unexpectedly");
            Err(io::Error::other("ChatGPT listener stopped unexpectedly"))
        },
    };

    state.stopping.send_replace(true);
    // Release the barrier before joining listeners: an accept-failure path may
    // also be waiting for this barrier. stopping already prevents new work.
    log::info!("waiting for accepted management commands before shutdown");
    drop(state.management.write().await);
    log::info!("stopping proxy listeners and draining connections");
    let (claude, chatgpt) = tokio::join!(claude.stop(), chatgpt.stop());
    // Prefer the original listener failure to a synthetic stopped notification.
    let result = claude.and(chatgpt).and(result);
    match &result {
        Ok(()) => log::info!("proxy listeners stopped"),
        Err(error) => diagnostics::error("proxy listeners shutdown", error),
    }
    result
}

/// Bind `address` and start proxying with a snapshot of `token`.
/// Upstream connectivity is checked per request, not during startup.
pub async fn start(address: &str, token: &str) -> io::Result<Proxy> {
    listen(address, Some(token)).await
}

/// Bind once, optionally enabling proxying. A disabled listener returns HTTP 503
/// until enabled through [`Proxy::set_token`]. No management endpoints are served.
/// System TLS roots are loaded once, including when initially disabled.
pub async fn listen(address: &str, token: Option<&str>) -> io::Result<Proxy> {
    if let Some(token) = token {
        validate_token(token)?;
    }
    let tls = tokio::task::spawn_blocking(upstream::tls_config)
        .await
        .map_err(io::Error::other)??;
    let slot = Arc::new(ProxySlot::default());
    slot.set_token(token, &tls).await?;
    let listener = TcpListener::bind(address).await?;
    Ok(spawn(listener, slot, tls, None))
}

fn validate_token(token: &str) -> io::Result<()> {
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CoreInfra token is not set",
        ));
    }
    Ok(())
}

fn spawn(
    listener: TcpListener,
    slot: Arc<ProxySlot>,
    tls: ClientConfig,
    management: Option<Arc<ServerState>>,
) -> Proxy {
    let (shutdown, receiver) = oneshot::channel();
    let (finished, stopped) = watch::channel(());
    let state = Arc::clone(&slot);
    let task = tokio::spawn(async move {
        // Dropping the only sender wakes observers on every task-exit path.
        let _finished = finished;
        serve(listener, state, management, receiver).await
    });
    Proxy {
        shutdown,
        task,
        stopped,
        slot,
        tls,
    }
}

async fn serve(
    listener: TcpListener,
    slot: Arc<ProxySlot>,
    management: Option<Arc<ServerState>>,
    mut shutdown: oneshot::Receiver<()>,
) -> io::Result<()> {
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    diagnostics::error("proxy connection task", &error);
                }
            },
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(http::serve_client(
                            stream, Arc::clone(&slot), management.clone(),
                        ));
                    }
                    Err(error) => {
                        diagnostics::error("proxy listener accept", &error);
                        break Err(error);
                    }
                }
            }
        }
    };

    drop(listener);
    // An accept failure must not cancel a Reload already accepted on this port.
    // Normal shutdown is quiesced by run_listeners before stop() is called.
    if result.is_err()
        && let Some(state) = &management
    {
        state.stopping.send_replace(true);
        drop(state.management.write().await);
    }
    connections.abort_all();
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result
            && !error.is_cancelled()
        {
            diagnostics::error("proxy connection task during shutdown", &error);
        }
    }
    result
}

async fn run_proxy_session(generation: Arc<Generation>, session: impl Future<Output = ()>) {
    let active = generation.active.read().await;
    {
        // Own the future in this inner scope: even on cancellation, its sockets
        // must be dropped before releasing the active-session guard below.
        let session = session;
        let mut cancelled = generation.cancelled.subscribe();
        let is_cancelled = *cancelled.borrow();
        if is_cancelled {
            drop(session);
        } else {
            tokio::select! {
                biased;
                _ = cancelled.changed() => {},
                () = session => {},
            }
        }
    }
    drop(active);
}
