//! Tauri commands for agent discovery and configuration.

use crate::{revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use wizard_core::agents::{
    AgentStates, Backup, BackupAgent, claude, codex, collect_agent_state, opencode, pi,
};
use wizard_core::validate_token;

#[derive(Clone)]
pub(crate) struct AgentRevisionState;

#[tauri::command]
pub(crate) async fn wait_for_update(
    last_revision: u32,
    signal: tauri::State<'_, RevisionSignal<AgentRevisionState>>,
) -> Result<u32, String> {
    signal
        .wait(Some(last_revision))
        .await
        .map(|snapshot| snapshot.revision)
        .map_err(|error| format!("{error:#}"))
}

#[derive(Serialize)]
pub(crate) struct AgentStateSnapshot {
    revision: u32,
    #[serde(flatten)]
    state: AgentStates,
}

#[tauri::command]
pub(crate) async fn get_agent_state(app: tauri::AppHandle) -> Result<AgentStateSnapshot, String> {
    let revision = app
        .state::<RevisionSignal<AgentRevisionState>>()
        .current()
        .revision;
    log::debug!("collecting agent state at revision {revision}");
    let settings = settings::get_state(&app).map_err(|error| format!("{error:#}"))?;
    let state = collect_agent_state(&settings).await;

    let detected = state.clone();
    settings::update_state(&app, move |settings| {
        detected.update_settings_paths(settings);
    })
    .await
    .map_err(|error| format!("failed to save discovered agent paths: {error:#}"))?;

    log::debug!("agent state collected at revision {revision}");
    Ok(AgentStateSnapshot { revision, state })
}

#[tauri::command]
pub(crate) async fn get_agent_backups(agent: BackupAgent) -> Result<Vec<Backup>, String> {
    // TODO: files can make this response large and include credentials.
    // Consider loading file contents on demand if backup inspection is added to the UI.
    // Never log the response.
    tauri::async_runtime::spawn_blocking(move || match agent {
        BackupAgent::Codex => codex::get_backups(),
        BackupAgent::Claude => claude::get_backups(),
    })
    .await
    .context("backup listing task failed")
    .flatten()
    .map_err(|error| format!("{error:#}"))
}

// No Debug: event payloads may contain credentials.
#[derive(Deserialize)]
pub(crate) enum AgentEvent {
    CodexSetProxy(codex::ProxyMode),
    ClaudeSetProxy(claude::ProxyMode),
    RestoreBackup { agent: BackupAgent, id: u32 },
    SetPiHub(bool),
    SetOpenCodeHub(bool),
    SetCoreInfraToken(String),
}

#[tauri::command]
pub(crate) async fn agent_event(event: AgentEvent, app: tauri::AppHandle) -> Result<(), String> {
    let name = match &event {
        AgentEvent::CodexSetProxy(_) => "CodexSetProxy",
        AgentEvent::ClaudeSetProxy(_) => "ClaudeSetProxy",
        AgentEvent::RestoreBackup { .. } => "RestoreBackup",
        AgentEvent::SetPiHub(_) => "SetPiHub",
        AgentEvent::SetOpenCodeHub(_) => "SetOpenCodeHub",
        AgentEvent::SetCoreInfraToken(_) => "SetCoreInfraToken",
    };
    log::info!("received agent event: {name}");
    let result = apply_event(event, &app).await;
    app.state::<RevisionSignal<AgentRevisionState>>()
        .notify(AgentRevisionState);
    if let Err(error) = result {
        log::error!("agent event {name} failed: {error:#}");
        return Err(format!("{error:#}"));
    }
    log::info!("agent event completed: {name}");
    Ok(())
}

async fn apply_event(event: AgentEvent, app: &tauri::AppHandle) -> Result<()> {
    match event {
        AgentEvent::SetCoreInfraToken(token) => {
            validate_token(&token)?;
            settings::update_state(app, move |settings| {
                settings.coreinfra_token = token;
            })
            .await
        }
        AgentEvent::CodexSetProxy(mode) => {
            let current = settings::get_state(app)?;
            tauri::async_runtime::spawn_blocking(move || {
                codex::set_proxy(mode, &current.coreinfra_token)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
        AgentEvent::ClaudeSetProxy(mode) => {
            let current = settings::get_state(app)?;
            tauri::async_runtime::spawn_blocking(move || {
                claude::set_proxy(mode, &current.coreinfra_token)
            })
            .await
            .context("agent event task failed")
            .flatten()
        }
        AgentEvent::RestoreBackup { agent, id } => {
            tauri::async_runtime::spawn_blocking(move || match agent {
                BackupAgent::Codex => codex::restore_backup(id),
                BackupAgent::Claude => claude::restore_backup(id),
            })
            .await
            .context("backup restore task failed")
            .flatten()
        }
        AgentEvent::SetOpenCodeHub(install) => {
            let current = settings::get_state(app)?;
            tauri::async_runtime::spawn_blocking(move || opencode::set_hub(install, &current))
                .await
                .context("agent event task failed")
                .flatten()
        }
        AgentEvent::SetPiHub(install) => {
            let current = settings::get_state(app)?;
            tauri::async_runtime::spawn_blocking(move || pi::set_hub(install, &current))
                .await
                .context("agent event task failed")
                .flatten()
        }
    }
}
