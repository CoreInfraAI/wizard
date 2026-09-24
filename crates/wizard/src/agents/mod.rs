//! Agent discovery and Tauri commands.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;

use crate::revision_signal::RevisionSignal;

mod codex_cli;
mod codex_desktop;

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) enum AgentEvent {
    CodexCliInstall,
    CodexCliUninstall,
}

#[tauri::command]
pub(crate) async fn agent_event(event: AgentEvent, app: tauri::AppHandle) -> Result<(), String> {
    log::info!("received agent event: {event:?}");
    let result = tauri::async_runtime::spawn_blocking(move || match event {
        AgentEvent::CodexCliInstall => codex_cli::set_proxy(true),
        AgentEvent::CodexCliUninstall => codex_cli::set_proxy(false),
    })
    .await
    .context("agent event task failed")
    .and_then(core::convert::identity);
    if let Err(error) = result {
        log::error!("agent event {event:?} failed: {error:#}");
        return Err(format!("{error:#}"));
    }
    log::info!("agent event completed: {event:?}");
    app.state::<RevisionSignal>().notify();
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStateSnapshot {
    revision: String,
    agents: AgentStates,
}

#[tauri::command]
pub(crate) async fn get_agent_state(app: tauri::AppHandle) -> AgentStateSnapshot {
    let revision = app.state::<RevisionSignal>().current().to_string();
    log::debug!("collecting agent state at revision {revision}");
    let agents = detect().await;
    log::debug!("agent state collected at revision {revision}");
    AgentStateSnapshot { revision, agents }
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
    pub codex_cli: AgentDetection<codex_cli::CodexCli>,
    pub codex_desktop: AgentDetection<codex_desktop::CodexDesktop>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum AgentDetection<T> {
    Found(T),
    NotFound,
    Error(String),
}

#[cfg_attr(any(target_os = "linux", target_os = "windows"), expect(dead_code))]
impl<T> AgentDetection<T> {
    fn failed(path: &std::path::Path, error: &impl core::fmt::Display) -> Self {
        Self::Error(format!("{}: {error:#}", path.display()))
    }
}
