use block2::RcBlock;
use core::time::Duration;
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSError, NSString};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::{io, process::Stdio, sync::mpsc};
use tokio::process::Command;

const SERVICE_LABEL: &str = "ai.coreinfra.wizard.proxy";
const PLIST_NAME: &str = "ai.coreinfra.wizard.proxy.plist";

pub async fn start_daemon() -> io::Result<()> {
    // Do not trust a UID environment variable or infer identity from file owners.
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("/usr/bin/id")
            .arg("-u")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "current user lookup timed out"))??;
    if !output.status.success() {
        return Err(io::Error::other("failed to determine current user"));
    }
    let uid = core::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .ok_or_else(|| io::Error::other("invalid current user identifier"))?;

    // Registration may already have launched the agent. Without -k, kickstart
    // does not kill an existing instance. Never bypass approval via enable or
    // launch the executable directly as a fallback.
    let status = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("/bin/launchctl")
            .arg("kickstart")
            .arg(format!("gui/{uid}/{SERVICE_LABEL}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "proxy service start request timed out",
        )
    })??;
    if !status.success() {
        return Err(io::Error::other(
            "launchd rejected the proxy service start request",
        ));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "SMAppService bindings require checked native calls"
)]
pub fn create_daemon() -> io::Result<()> {
    if !objc2::available!(macos = 13.0) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "proxy service registration requires macOS 13 or later",
        ));
    }

    autoreleasepool(|_| {
        let name = NSString::from_str(PLIST_NAME);
        // SAFETY: macOS availability is checked above. The fixed name identifies
        // our bundled LaunchAgent, not arbitrary caller-supplied service data.
        // The returned service is retained for every subsequent native call.
        let service = unsafe { SMAppService::agentServiceWithPlistName(&name) };
        // SAFETY: service is a live SMAppService on a supported OS.
        let mut status = unsafe { service.status() };
        log::info!("proxy service status before registration: {}", status.0);
        if status == SMAppServiceStatus::Enabled {
            log::info!("unregistering proxy service and waiting for termination");
            let (finished, completion) = mpsc::sync_channel(1);
            let handler = RcBlock::new(move |error: *mut NSError| {
                // SAFETY: SMAppService supplies either null or an NSError valid
                // during this callback. Only its numeric code leaves the callback.
                let result = unsafe { error.as_ref() }.map_or(Ok(()), |error| {
                    Err(io::Error::other(format!(
                        "proxy service unregistration failed (code {})",
                        error.code(),
                    )))
                });
                let _ = finished.try_send(result);
            });
            // SAFETY: service is live; SMAppService retains the escaping block
            // and invokes it on a dispatch queue after termination or an error.
            unsafe { service.unregisterWithCompletionHandler(&handler) };
            completion
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => io::Error::new(
                        io::ErrorKind::TimedOut,
                        "proxy service unregistration timed out",
                    ),
                    mpsc::RecvTimeoutError::Disconnected => {
                        io::Error::other("proxy service unregistration callback was lost")
                    }
                })??;
            // Never register before successful completion, including on timeout:
            // the native unregistration may still finish after we return an error.
            log::info!("proxy service unregistration completed");
            status = SMAppServiceStatus::NotRegistered;
        }
        if status == SMAppServiceStatus::NotRegistered || status == SMAppServiceStatus::NotFound {
            // NotFound may mean the background task record is missing. Attempt
            // registration so its result, rather than the initial status, decides.
            log::info!("registering bundled proxy LaunchAgent");
            // SAFETY: the service and its fixed plist name remain valid. This is
            // the registration step, before requesting launch through launchd.
            let registration = unsafe { service.registerAndReturnError() };
            // SAFETY: service remains alive after the registration attempt.
            status = unsafe { service.status() };
            log::info!("proxy service status after registration: {}", status.0);
            if status != SMAppServiceStatus::Enabled
                && status != SMAppServiceStatus::RequiresApproval
            {
                // Keep diagnostics useful without exposing NSError descriptions.
                registration.map_err(|error| {
                    io::Error::other(format!(
                        "proxy service registration failed (code {})",
                        error.code(),
                    ))
                })?;
            }
        }

        match status {
            SMAppServiceStatus::Enabled => Ok(()),
            SMAppServiceStatus::RequiresApproval => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "proxy service requires approval in System Settings > General > Login Items",
            )),
            SMAppServiceStatus::NotFound => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "proxy LaunchAgent was not found by SMAppService",
            )),
            SMAppServiceStatus::NotRegistered => {
                Err(io::Error::other("proxy service is not registered"))
            }
            _ => Err(io::Error::other(
                "unknown proxy service registration status",
            )),
        }
    })
}
