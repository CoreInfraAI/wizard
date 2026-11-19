use core::time::Duration;
use std::{io, process::Stdio};
use tokio::{process::Command, time::timeout};

pub(super) use super::unix::shutdown_signal;

const UNIT: &str = "wizard-proxy.service";

async fn systemctl(args: &[&str]) -> io::Result<std::process::ExitStatus> {
    let mut command = Command::new("/usr/bin/systemctl");
    command
        .args(["--user", "--no-ask-password"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    timeout(Duration::from_secs(15), command.status()).await?
}

fn check(status: std::process::ExitStatus, operation: &str) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("systemctl {operation}: {status}")))
    }
}

// dpkg owns the unit file. Stop the process, but never delete the unit here.
pub(super) async fn uninstall() -> io::Result<bool> {
    let status = systemctl(&["stop", UNIT]).await?;
    if status.code() == Some(5) {
        return Ok(false); // No such unit (e.g. a source-only installation).
    }
    check(status, "stop")?;
    Ok(true)
}

pub(super) async fn create_daemon() -> io::Result<()> {
    ensure_daemon().await?;
    // Drop a stale service process before the caller synchronizes saved demand.
    check(systemctl(&["stop", UNIT]).await?, "stop")
}

pub(super) async fn ensure_daemon() -> io::Result<()> {
    check(systemctl(&["daemon-reload"]).await?, "daemon-reload")
}

pub(super) async fn start_daemon() -> io::Result<()> {
    check(systemctl(&["start", UNIT]).await?, "start")
}
