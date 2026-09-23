//! Read-only agent discovery and its Tauri command.

use std::path::PathBuf;

use serde::Serialize;

mod codex_cli;
mod codex_desktop;

#[tauri::command]
pub(crate) async fn get_agent_state() -> AgentStates {
    detect().await
}

async fn detect() -> AgentStates {
    // Start all detectors before awaiting their results so they can run concurrently.
    let codex_cli = tauri::async_runtime::spawn_blocking(codex_cli::detect);
    let codex_desktop = tauri::async_runtime::spawn_blocking(codex_desktop::detect);

    AgentStates {
        codex_cli: codex_cli
            .await
            .unwrap_or_else(|error| AgentDetection::Error(error.to_string())),
        codex_desktop: codex_desktop
            .await
            .unwrap_or_else(|error| AgentDetection::Error(error.to_string())),
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStates {
    pub codex_cli: AgentDetection,
    pub codex_desktop: AgentDetection,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum AgentDetection {
    Found(Installation),
    NotFound,
    Error(String),
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Installation {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[cfg(target_os = "macos")]
impl AgentDetection {
    fn found(path: PathBuf, version: Option<String>) -> Self {
        Self::Found(Installation { path, version })
    }

    fn failed(path: &std::path::Path, error: &impl core::fmt::Display) -> Self {
        Self::Error(format!("{}: {error}", path.display()))
    }
}
