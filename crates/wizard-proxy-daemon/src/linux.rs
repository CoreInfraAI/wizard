use std::io;

pub(super) use super::unix::shutdown_signal;

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
