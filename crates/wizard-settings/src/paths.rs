//! Shared configuration and log locations used by Wizard and its proxy.

use anyhow::{Context as _, Result};
use std::path::PathBuf;

// Match `identifier` in crates/wizard-gui/tauri.conf.json.
const IDENTIFIER: &str = "ai.coreinfra.wizard";

pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join(IDENTIFIER))
        .context("failed to resolve Wizard settings directory")
}

/// Data Wizard can recreate, which the system may delete. Does not create the directory.
pub fn cache_dir() -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|dir| dir.join(IDENTIFIER))
        .context("failed to resolve Wizard cache directory")
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

/// Claude's local profile library and the adjacent Wizard-owned proxy credential.
pub fn claude_config_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let directory = dirs::home_dir().map(|home| home.join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let directory = dirs::data_local_dir();
    #[cfg(target_os = "linux")]
    let directory = dirs::config_dir();
    std::path::absolute(
        directory
            .context("failed to resolve Claude Desktop configuration directory")?
            .join("Claude-3p"),
    )
    .context("failed to resolve Claude Desktop configuration path")
}

pub fn claude_proxy_token_path() -> Result<PathBuf> {
    Ok(claude_config_dir()?.join("coreinfra-proxy-token"))
}

/// Matches Tauri's `app_log_dir`; does not create the directory.
pub fn logs_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let path = dirs::home_dir().map(|dir| dir.join("Library/Logs").join(IDENTIFIER));
    #[cfg(not(target_os = "macos"))]
    let path = dirs::data_local_dir().map(|dir| dir.join(IDENTIFIER).join("logs"));

    path.context("failed to resolve Wizard logs directory")
}
