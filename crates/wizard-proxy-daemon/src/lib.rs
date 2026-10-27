use core::time::Duration;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::process::ExitCode;
use tokio::sync::Mutex;

mod logging;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn create_daemon() -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "proxy service registration is not implemented on this platform",
        ))
    }

    pub(super) async fn start_daemon() -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "proxy service startup is not implemented on this platform",
        ))
    }
}

/// Prepare the service once at GUI startup, before synchronizing settings.
/// In release builds, a running daemon with the same workspace crate version is
/// left untouched. Debug builds ignore version matches to pick up local rebuilds.
/// Otherwise a valid Status or refused connection triggers platform registration
/// on a blocking worker. Other HTTP errors leave registration unchanged.
pub async fn create_daemon() -> io::Result<()> {
    let guard = SYNC_LOCK.lock().await;
    match wizard_proxy::control::status().await {
        Ok(status) if !cfg!(debug_assertions) && status.version == env!("CARGO_PKG_VERSION") => {
            log::info!("proxy daemon version matches; keeping current service");
            return Ok(());
        }
        Ok(_) => log::info!("preparing proxy service registration: version differs or debug build"),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            log::info!("preparing proxy service registration: daemon is not listening");
        }
        Err(error) => return Err(error),
    }

    tokio::task::spawn_blocking(move || {
        // Keep registration serialized even if the awaiting caller is cancelled.
        let _guard = guard;
        let result = platform::create_daemon();
        match &result {
            Ok(()) => log::info!("proxy service registration ready"),
            Err(error) => log::error!("proxy service registration failed: kind={:?}", error.kind()),
        }
        result
    })
    .await
    .map_err(|_| io::Error::other("proxy service registration task failed"))?
}

static SYNC_LOCK: Mutex<()> = Mutex::const_new(());

/// Synchronize the independent proxy process with saved settings.
/// Only a refused connection permits starting a process: a timeout or failed
/// Reload may already have applied changes and must not trigger a second launch.
/// A single platform start request is followed by bounded Status polling.
/// This never registers the service; call `create_daemon` at GUI startup first.
/// Neither a launch nor a mutating Reload is automatically retried.
pub async fn sync_proxy() -> io::Result<()> {
    let _guard = SYNC_LOCK.lock().await;
    log::info!("synchronizing proxy daemon with saved settings");
    match wizard_proxy::control::reload().await {
        Ok(response) => {
            if let Some(error) = response.error {
                Err(io::Error::other(error))
            } else {
                log::info!("proxy daemon settings synchronized");
                Ok(())
            }
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            let enabled = wizard_settings::load_from_file()
                .map_err(|_| io::Error::other("failed to load Wizard settings"))?
                .local_proxy_enabled();
            if enabled {
                log::info!("requesting proxy service start");
                platform::start_daemon().await?;
                log::info!("waiting for proxy daemon readiness");
                wait_for_proxy().await?;
            } else {
                log::info!("proxy service start skipped: both proxies are disabled");
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

async fn wait_for_proxy() -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match wizard_proxy::control::status().await {
                Ok(_) => {
                    log::info!("proxy daemon HTTP management is ready");
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    // Settings may have been disabled while startup
                    // was pending. In that case a clean daemon exit is expected.
                    if !wizard_settings::load_from_file()
                        .map_err(|_| io::Error::other("failed to load Wizard settings"))?
                        .local_proxy_enabled()
                    {
                        log::info!("proxy readiness wait ended: both proxies are now disabled");
                        return Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy daemon did not become ready"))?
}

/// Run the proxy process without initializing the GUI.
/// Call once at process startup, before any other logger or Tokio runtime.
#[must_use]
pub fn main() -> ExitCode {
    let result = (|| {
        let _instance = acquire_instance_lock()?;
        logging::initialize()?;
        log::info!(
            "proxy daemon starting: version={}, pid={}",
            env!("CARGO_PKG_VERSION"),
            std::process::id(),
        );
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "failed to create proxy runtime")
            .and_then(|runtime| runtime.block_on(run()));
        match &result {
            Ok(()) => log::info!("proxy daemon stopped"),
            Err(operation) => log::error!("{operation}"),
        }
        log::logger().flush();
        result
    })();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(operation) => {
            // Only static diagnostics: never print settings, tokens or raw errors.
            eprintln!("{operation}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    // Register handlers before starting listeners so startup-time events remain pending.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let shutdown = {
        use tokio::signal::unix::{SignalKind, signal};

        let mut interrupt =
            signal(SignalKind::interrupt()).map_err(|_| "failed to register SIGINT handler")?;
        let mut terminate =
            signal(SignalKind::terminate()).map_err(|_| "failed to register SIGTERM handler")?;
        async move {
            tokio::select! {
                _ = interrupt.recv() => log::info!("proxy daemon received SIGINT"),
                _ = terminate.recv() => log::info!("proxy daemon received SIGTERM"),
            }
        }
    };
    #[cfg(windows)]
    let shutdown = {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};

        let mut interrupt = ctrl_c().map_err(|_| "failed to register Ctrl+C handler")?;
        let mut stop = ctrl_break().map_err(|_| "failed to register Ctrl+Break handler")?;
        let mut close = ctrl_close().map_err(|_| "failed to register console-close handler")?;
        async move {
            tokio::select! {
                _ = interrupt.recv() => log::info!("proxy daemon received Ctrl+C"),
                _ = stop.recv() => log::info!("proxy daemon received Ctrl+Break"),
                _ = close.recv() => log::info!("proxy daemon received console-close"),
            }
        }
    };
    wizard_proxy::run_managed(shutdown)
        .await
        .map_err(|_| "proxy server failed")
}

/// Keep the returned file open until shutdown. Never unlink the lock file: another
/// process could otherwise lock a different file at the same path.
fn acquire_instance_lock() -> Result<File, &'static str> {
    let directory = wizard_settings::paths::config_dir()
        .map_err(|_| "failed to resolve proxy lock directory")?;
    fs::create_dir_all(&directory).map_err(|_| "failed to create proxy lock directory")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(directory.join("proxy-daemon.lock"))
        .map_err(|_| "failed to open proxy lock file")?;
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => "proxy daemon is already running",
        TryLockError::Error(_) => "failed to lock proxy instance",
    })?;
    Ok(file)
}
