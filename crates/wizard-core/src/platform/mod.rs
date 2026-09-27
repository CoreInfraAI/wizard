use core::time::Duration;
use std::{path::Path, process::Output};

use anyhow::{Context as _, Result};

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// Runs a command from a Tokio blocking worker with bounded execution time.
/// Must be called inside `spawn_blocking`.
pub(crate) fn command_output(program: &Path, args: &[&str]) -> Result<Output> {
    let runtime =
        tokio::runtime::Handle::try_current().context("command_output requires a Tokio runtime")?;
    log::debug!("running command: {}, args: {args:?}", program.display());
    runtime.block_on(async {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .with_context(|| format!("{} timed out after 5 seconds", program.display()))?
            .with_context(|| format!("failed to run {}", program.display()))
    })
}
