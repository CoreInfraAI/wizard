//! Agent discovery and Tauri commands.

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;

use crate::{revision_signal::RevisionSignal, settings};

mod codex_cli;
mod codex_desktop;

#[derive(Debug, Serialize)]
pub(crate) struct AgentStateSnapshot {
    revision: String,
    #[serde(flatten)]
    state: AgentState,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentState {
    agents: AgentStates,
    coreinfra_token_set: bool,
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

#[tauri::command]
pub(crate) async fn get_agent_state(app: tauri::AppHandle) -> Result<AgentStateSnapshot, String> {
    let revision = app.state::<RevisionSignal>().current().to_string();
    log::debug!("collecting agent state at revision {revision}");
    let settings = settings::get_state(&app)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let state = collect_agent_state(&settings).await;
    log::debug!("agent state collected at revision {revision}");
    Ok(AgentStateSnapshot { revision, state })
}

/// Collects state for GUI or CLI without requiring a running Tauri application.
pub(crate) async fn collect_agent_state(settings: &settings::Settings) -> AgentState {
    let agents = detect().await;
    let coreinfra_token_set = !settings.coreinfra_api_key.is_empty();

    AgentState {
        agents,
        coreinfra_token_set,
    }
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

// No Debug: event payloads may contain credentials.
#[derive(Deserialize)]
pub(crate) enum AgentEvent {
    CodexCliInstall,
    CodexCliUninstall,
    SetCoreinfraToken(String),
}

#[tauri::command]
pub(crate) async fn agent_event(event: AgentEvent, app: tauri::AppHandle) -> Result<(), String> {
    let name = match &event {
        AgentEvent::CodexCliInstall => "CodexCliInstall",
        AgentEvent::CodexCliUninstall => "CodexCliUninstall",
        AgentEvent::SetCoreinfraToken(_) => "SetCoreinfraToken",
    };
    log::info!("received agent event: {name}");
    let result = apply_event(event, &app).await;
    app.state::<RevisionSignal>().notify();
    if let Err(error) = result {
        log::error!("agent event {name} failed: {error:#}");
        return Err(format!("{error:#}"));
    }
    log::info!("agent event completed: {name}");
    Ok(())
}

async fn apply_event(event: AgentEvent, app: &tauri::AppHandle) -> Result<()> {
    match event {
        AgentEvent::SetCoreinfraToken(token) => {
            settings::update_state(app, move |settings| {
                settings.coreinfra_api_key = token;
            })
            .await
        }
        AgentEvent::CodexCliInstall | AgentEvent::CodexCliUninstall => {
            let installed = matches!(event, AgentEvent::CodexCliInstall);
            let current = settings::get_state(app).await?;
            tauri::async_runtime::spawn_blocking(move || {
                codex_cli::set_proxy(installed, &current.coreinfra_api_key)
            })
            .await
            .context("agent event task failed")?
        }
    }
}
