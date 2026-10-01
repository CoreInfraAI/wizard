//! Tauri commands for agent discovery and configuration.

use crate::{revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use wizard_core::agents::{AgentState, claude, codex, collect_agent_state, opencode, pi};
use wizard_core::validate_token;

#[derive(Debug, Serialize)]
pub(crate) struct AgentStateSnapshot {
    revision: String,
    #[serde(flatten)]
    state: AgentState,
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

// No Debug: event payloads may contain credentials.
#[derive(Deserialize)]
pub(crate) enum AgentEvent {
    CodexSetProxy(codex::ProxyMode),
    ClaudeSetProxy(claude::ProxyMode),
    SetPiHub(bool),
    SetOpenCodeHub(bool),
    SetCoreinfraToken(String),
}

#[tauri::command]
pub(crate) async fn agent_event(event: AgentEvent, app: tauri::AppHandle) -> Result<(), String> {
    let name = match &event {
        AgentEvent::CodexSetProxy(_) => "CodexSetProxy",
        AgentEvent::ClaudeSetProxy(_) => "ClaudeSetProxy",
        AgentEvent::SetPiHub(_) => "SetPiHub",
        AgentEvent::SetOpenCodeHub(_) => "SetOpenCodeHub",
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
            validate_token(&token)?;
            settings::update_state(app, move |settings| {
                settings.coreinfra_api_key = token;
            })
            .await
        }
        AgentEvent::CodexSetProxy(mode) => {
            let current = settings::get_state(app).await?;
            tauri::async_runtime::spawn_blocking(move || {
                codex::set_proxy(mode, &current.coreinfra_api_key)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
        AgentEvent::ClaudeSetProxy(mode) => {
            let current = settings::get_state(app).await?;
            tauri::async_runtime::spawn_blocking(move || {
                claude::set_proxy(mode, &current.coreinfra_api_key)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
        AgentEvent::SetOpenCodeHub(install) => {
            let current = settings::get_state(app).await?;
            tauri::async_runtime::spawn_blocking(move || {
                opencode::set_hub(install, &current.coreinfra_api_key)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
        AgentEvent::SetPiHub(install) => {
            let current = settings::get_state(app).await?;
            tauri::async_runtime::spawn_blocking(move || {
                pi::set_hub(install, &current.coreinfra_api_key)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
    }
}
