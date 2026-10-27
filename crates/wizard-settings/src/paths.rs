//! Shared locations of Wizard's own configuration and log files.

use anyhow::{Context as _, Result};
use std::path::PathBuf;

// Match `identifier` in crates/wizard-gui/tauri.conf.json.
const IDENTIFIER: &str = "ai.coreinfra.wizard";

pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join(IDENTIFIER))
        .context("failed to resolve Wizard settings directory")
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Matches Tauri's `app_log_dir`; does not create the directory.
pub fn logs_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let path = dirs::home_dir().map(|dir| dir.join("Library/Logs").join(IDENTIFIER));
    #[cfg(not(target_os = "macos"))]
    let path = dirs::data_local_dir().map(|dir| dir.join(IDENTIFIER).join("logs"));

    path.context("failed to resolve Wizard logs directory")
}
