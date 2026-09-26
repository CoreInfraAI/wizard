use core::time::Duration;
use std::{path::Path, process::Output};

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// Runs a command on a blocking worker with bounded execution time.
/// The caller interprets its exit status and output.
pub(crate) fn command_output(program: &Path, args: &[&str]) -> Result<Output> {
    log::debug!("running command: {}, args: {args:?}", program.display());
    tauri::async_runtime::block_on(async {
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

#[derive(Deserialize)]
struct AppInfo {
    #[serde(rename = "CFBundleShortVersionString")]
    version: Option<String>,
}

/// Reads an app bundle's version without launching the application.
/// `plist` handles both XML and binary Info.plist files.
pub(crate) fn read_app_version(app: &Path) -> Result<Option<String>> {
    let path = app.join("Contents/Info.plist");
    log::debug!("reading application metadata: {}", path.display());
    let info: AppInfo =
        plist::from_file(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(info.version.filter(|version| !version.trim().is_empty()))
}
