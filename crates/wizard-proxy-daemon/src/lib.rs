use core::time::Duration;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::process::ExitCode;
use tokio::sync::Mutex;
use wizard_proxy::diagnostics;

mod logging;

#[cfg(unix)]
mod unix;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(any(windows, test))]
mod windows_task;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

/// Prepare the service once at GUI startup, before synchronizing settings.
/// In release builds, a running daemon with the same workspace crate version is
/// left untouched. Debug builds ignore version matches to pick up local rebuilds.
/// Otherwise a valid Status or refused connection triggers platform registration.
/// Other HTTP errors leave registration unchanged.
/// Await this operation to completion: cancellation can leave native registration
/// running after the synchronization lock is released.
pub async fn create_daemon() -> io::Result<()> {
    let _guard = SYNC_LOCK.lock().await;
    match wizard_proxy::control::status().await {
        Ok(status) if !cfg!(debug_assertions) && status.version == env!("CARGO_PKG_VERSION") => {
            log::info!("proxy daemon version matches; keeping current service");
            return Ok(());
        }
        Ok(_) => log::info!("preparing proxy service registration: version differs or debug build"),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            log::info!("preparing proxy service registration: daemon is not listening");
        }
        Err(error) => {
            diagnostics::error("proxy version lookup before registration", &error);
            return Err(error);
        }
    }

    let result = platform::create_daemon().await;
    match &result {
        Ok(()) => log::info!("proxy service registration ready"),
        Err(error) => diagnostics::error("proxy service registration", error),
    }
    result
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
    let result = sync_with_settings().await;
    if let Err(error) = &result {
        diagnostics::error("proxy settings synchronization", error);
    }
    result
}

async fn sync_with_settings() -> io::Result<()> {
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
                .inspect_err(|error| {
                    diagnostics::error("proxy startup demand load", error.as_ref());
                })
                .map_err(|_| io::Error::other("failed to load Wizard settings"))?
                .local_proxy_enabled();
            if enabled {
                log::info!("requesting proxy service start");
                platform::start_daemon()
                    .await
                    .inspect_err(|error| diagnostics::error("proxy service start", error))?;
                log::info!("waiting for proxy daemon readiness");
                wait_for_proxy()
                    .await
                    .inspect_err(|error| diagnostics::error("proxy readiness wait", error))?;
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
                        .inspect_err(|error| {
                            diagnostics::error("proxy readiness demand load", error.as_ref());
                        })
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

/// Hold for the GUI lifetime so uninstall cannot race GUI synchronization.
#[cfg(windows)]
pub fn gui_guard() -> io::Result<File> {
    let file = uninstall_lock_file()?;
    file.try_lock_shared().map_err(io::Error::from)?;
    Ok(file)
}

#[cfg(windows)]
fn uninstall_lock_file() -> io::Result<File> {
    let directory = wizard_settings::paths::config_dir()
        .map_err(|_| io::Error::other("failed to resolve uninstall lock directory"))?;
    fs::create_dir_all(&directory)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("proxy-uninstall.lock"))
}

/// Remove the current installation's Windows proxy task without changing settings.
#[cfg(windows)]
#[must_use]
pub fn uninstall() -> ExitCode {
    let result = (|| -> io::Result<()> {
        let guard = uninstall_lock_file()?;
        guard.try_lock().map_err(io::Error::from)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(windows::uninstall())
    })();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("proxy uninstall failed: {error}");
            ExitCode::FAILURE
        }
    }
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
            .inspect_err(|error| diagnostics::error("proxy runtime creation", error))
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
    let shutdown = platform::shutdown_signal()?;
    wizard_proxy::run_managed(shutdown)
        .await
        .inspect_err(|error| diagnostics::error("proxy server", error))
        .map_err(|_| "proxy server failed")
}

/// Keep the returned file open until shutdown. Never unlink the lock file: another
/// process could otherwise lock a different file at the same path.
fn acquire_instance_lock() -> Result<File, &'static str> {
    let directory = wizard_settings::paths::config_dir()
        .map_err(|_| "failed to resolve proxy lock directory")?;
    fs::create_dir_all(&directory)
        .inspect_err(|error| logging::report_stderr("proxy lock directory creation", error))
        .map_err(|_| "failed to create proxy lock directory")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(directory.join("proxy-daemon.lock"))
        .inspect_err(|error| logging::report_stderr("proxy lock file open", error))
        .map_err(|_| "failed to open proxy lock file")?;
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => "proxy daemon is already running",
        TryLockError::Error(error) => {
            logging::report_stderr("proxy instance lock", &error);
            "failed to lock proxy instance"
        }
    })?;
    Ok(file)
}
