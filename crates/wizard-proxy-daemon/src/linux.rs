#![expect(
    clippy::unused_async,
    reason = "Linux stubs retain the async contract shared with Windows and macOS"
)]

use std::io;

pub(super) use super::unix::shutdown_signal;

// No platform service is installed on Linux yet. Cleanup is a successful no-op.
pub(super) async fn uninstall() -> io::Result<bool> {
    Ok(false)
}

pub(super) async fn create_daemon() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "proxy service registration is not implemented on this platform",
    ))
}

pub(super) async fn start_daemon() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "proxy service startup is not implemented on this platform",
    ))
}
