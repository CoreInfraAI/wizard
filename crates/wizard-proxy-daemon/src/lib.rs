use core::time::Duration;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::process::ExitCode;
use tokio::sync::Mutex;
use wizard_proxy::diagnostics;

mod logging;

#[cfg(unix)]
mod limits;
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

/// Exclusive access to service lifecycle operations while replacing the application.
/// Keep this guard until restart, or consume it to recover after an installation error.
/// Unlike CLI uninstall, this is used inside a GUI holding the shared GUI lifetime lock.
#[must_use = "keep the guard alive throughout installation and until application restart"]
pub struct UpdateGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    restore_service: bool,
}

/// Stop and unregister the service after downloading and verifying an update.
/// Failure must abort installation. Await completion: native work may outlive cancellation.
pub async fn prepare_update() -> io::Result<UpdateGuard> {
    prepare_update_with(platform::uninstall()).await
}

async fn prepare_update_with(
    uninstall: impl Future<Output = io::Result<bool>>,
) -> io::Result<UpdateGuard> {
    let lock = SYNC_LOCK.lock().await;
    log::info!("preparing proxy service for application update");
    let restore_service = uninstall
        .await
        .inspect_err(|error| diagnostics::error("proxy update preparation", error))?;
    log::info!("proxy update preparation completed: registration_removed={restore_service}");
    Ok(UpdateGuard {
        _lock: lock,
        restore_service,
    })
}

impl UpdateGuard {
    /// Best-effort restoration from the bundle left by a failed installer.
    /// Never reacquire `SYNC_LOCK` here: this guard already owns it.
    pub async fn restore_after_failure(self) -> io::Result<()> {
        self.restore_with(async {
            platform::create_daemon().await?;
            sync_with_settings().await
        })
        .await
    }

    async fn restore_with(self, restore: impl Future<Output = io::Result<()>>) -> io::Result<()> {
        if !self.restore_service {
            log::info!("proxy recovery skipped: no platform service was removed");
            return Ok(());
        }
        log::info!("restoring proxy service after failed application update");
        let result = restore.await;
        match &result {
            Ok(()) => log::info!("proxy service restored after failed application update"),
            Err(error) => {
                diagnostics::error("proxy recovery after failed application update", error);
            }
        }
        result
    }
}

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
                log::info!("proxy service start skipped: no consumers are enabled");
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
                        log::info!("proxy readiness wait ended: no consumers are enabled");
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
pub fn gui_guard() -> io::Result<File> {
    let file = uninstall_lock_file()?;
    file.try_lock_shared().map_err(io::Error::from)?;
    Ok(file)
}

fn uninstall_lock_file() -> io::Result<File> {
    let directory = wizard_settings::paths::config_dir()
        .map_err(|_| io::Error::other("failed to resolve uninstall lock directory"))?;
    fs::create_dir_all(&directory)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(directory.join("proxy-uninstall.lock"))
}

/// Remove the current installation's proxy service without changing settings.
/// On Windows, first remove Wizard's system proxy entries: nothing listens afterwards.
/// On macOS, run from the application bundle that registered the service.
/// On macOS and Windows, close GUI instances first. Linux is a successful no-op.
#[must_use]
pub fn uninstall() -> ExitCode {
    let result = (|| -> io::Result<()> {
        #[cfg(any(target_os = "macos", windows))]
        let _guard = {
            let guard = uninstall_lock_file()?;
            guard.try_lock().map_err(io::Error::from)?;
            guard
        };
        // Under the lock: a running GUI must not observe or change the proxy meanwhile.
        #[cfg(windows)]
        wizard_system_proxy::disable()
            .map_err(|error| io::Error::other(format!("system proxy cleanup failed: {error:#}")))?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(platform::uninstall())
            .map(|_| ())
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
        #[cfg(unix)]
        limits::raise_open_file_limit();
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

#[cfg(test)]
mod update_tests {
    use super::{SYNC_LOCK, prepare_update_with};
    use std::io;

    #[tokio::test]
    async fn update_guard_serializes_preparation_installation_and_recovery() {
        // Fake removal: no registration, process, or user settings are touched.
        let guard = prepare_update_with(async {
            assert!(SYNC_LOCK.try_lock().is_err(), "lock must precede removal");
            Ok(false)
        })
        .await
        .expect("prepare update");
        assert!(
            SYNC_LOCK.try_lock().is_err(),
            "installation must remain exclusive"
        );
        // The guard can be owned by a blocking installer without unlocking lifecycle operations.
        let guard = tokio::task::spawn_blocking(move || {
            assert!(SYNC_LOCK.try_lock().is_err());
            guard
        })
        .await
        .expect("fake installer");
        assert!(SYNC_LOCK.try_lock().is_err());
        // No service was removed: recovery must not call platform registration.
        guard
            .restore_with(async { panic!("must not create an absent service") })
            .await
            .expect("no-op recovery");
        assert!(SYNC_LOCK.try_lock().is_ok());

        let guard = prepare_update_with(async { Ok(true) })
            .await
            .expect("fake service removal");
        let error = guard
            .restore_with(async {
                assert!(
                    SYNC_LOCK.try_lock().is_err(),
                    "recovery must remain exclusive"
                );
                tokio::task::yield_now().await;
                assert!(SYNC_LOCK.try_lock().is_err());
                Err(io::Error::from(io::ErrorKind::ConnectionRefused))
            })
            .await
            .expect_err("recovery failure must be reported");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(SYNC_LOCK.try_lock().is_ok());

        let error = prepare_update_with(async {
            assert!(SYNC_LOCK.try_lock().is_err());
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .await
        .err()
        .expect("preparation must fail");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            SYNC_LOCK.try_lock().is_ok(),
            "failed preparation must release the lock"
        );
    }
}
