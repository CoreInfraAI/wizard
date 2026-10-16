//! Tauri commands for agent discovery and configuration.

use crate::{revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use wizard_core::agents::{
    AgentStates, Backup, BackupAgent, claude, claude_desktop, codex, collect_agent_state, opencode,
    pi,
};
use wizard_core::prepared_plan::{PreparedPlan, PreparedStepInfo};
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

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum AgentChange {
    CodexSetProxy(codex::ProxyMode),
    ClaudeSetProxy(claude::ProxyMode),
    ClaudeDesktopSetProxy(claude_desktop::ProxyMode),
    SetPiHub(bool),
    SetOpenCodeHub(bool),
}

// No Debug: the plan contains credentials; only info is intended for display.
#[derive(Serialize)]
pub(crate) struct PreparedAgentChange {
    change: AgentChange,
    agent: &'static str,
    plan: PreparedPlan,
    info: Vec<PreparedStepInfo>,
}

#[tauri::command]
pub(crate) async fn prepare_agent_change(
    change: AgentChange,
    app: tauri::AppHandle,
) -> Result<PreparedAgentChange, String> {
    let (name, agent) = match &change {
        AgentChange::CodexSetProxy(_) => ("CodexSetProxy", "codex"),
        AgentChange::ClaudeSetProxy(_) => ("ClaudeSetProxy", "claude"),
        AgentChange::ClaudeDesktopSetProxy(_) => ("ClaudeDesktopSetProxy", "claude_desktop"),
        AgentChange::SetPiHub(_) => ("SetPiHub", "pi"),
        AgentChange::SetOpenCodeHub(_) => ("SetOpenCodeHub", "opencode"),
    };
    log::info!("received agent change: {name}");

    let settings = settings::get_state(&app).map_err(|error| format!("{error:#}"))?;

    // Plans contain credentials: never log the returned value.
    let plan = tauri::async_runtime::spawn_blocking(move || match change {
        AgentChange::CodexSetProxy(mode) => codex::prepare_proxy(mode, &settings),
        AgentChange::ClaudeSetProxy(mode) => claude::prepare_proxy(mode, &settings),
        AgentChange::ClaudeDesktopSetProxy(mode) => claude_desktop::prepare_proxy(mode, &settings),
        AgentChange::SetPiHub(install) => pi::prepare_hub(install, &settings),
        AgentChange::SetOpenCodeHub(install) => opencode::prepare_hub(install, &settings),
    })
    .await
    .context("agent change preparation task failed")
    .flatten()
    .map_err(|error| {
        log::error!("agent change {name} preparation failed: {error:#}");
        format!("{error:#}")
    })?;

    let info = plan.info();

    log::info!("agent change prepared: {name}");
    Ok(PreparedAgentChange {
        change,
        agent,
        plan,
        info,
    })
}

#[tauri::command]
pub(crate) async fn apply_agent_plan(
    plan: PreparedPlan,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Only the trusted local frontend should return plans from prepare_agent_change.
    // Do not log the plan or recompute it from current settings.
    log::info!("applying agent plan");

    let proxy_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        plan.apply_with_claude_proxy(|enabled| {
            tauri::async_runtime::block_on(settings::update_state(&proxy_app, move |settings| {
                settings.claude_proxy_enable = enabled;
            }))
        })
    })
    .await
    .context("agent plan application task failed")
    .flatten();

    // A failed application may have changed files before the failing step.
    app.state::<RevisionSignal<AgentRevisionState>>()
        .notify(AgentRevisionState);

    result.map_err(|error| {
        log::error!("agent plan application failed: {error:#}");
        format!("{error:#}")
    })?;

    log::info!("agent plan applied");
    Ok(())
}

// No Debug: direct changes may contain credentials.
#[derive(Deserialize)]
pub(crate) enum DirectChange {
    SetCoreInfraToken(String),
    RestoreBackup { agent: BackupAgent, id: u32 },
}

#[tauri::command]
pub(crate) async fn apply_direct_change(
    change: DirectChange,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let name = match &change {
        DirectChange::SetCoreInfraToken(_) => "SetCoreInfraToken",
        DirectChange::RestoreBackup { .. } => "RestoreBackup",
    };
    log::info!("received direct change: {name}");

    let result = match change {
        DirectChange::SetCoreInfraToken(token) => {
            validate_token(&token).map_err(|error| format!("{error:#}"))?;
            settings::update_state(&app, move |settings| settings.coreinfra_token = token).await
        }
        DirectChange::RestoreBackup { agent, id } => {
            let result = tauri::async_runtime::spawn_blocking(move || match agent {
                BackupAgent::Codex => codex::restore_backup(id),
                BackupAgent::Claude => claude::restore_backup(id),
            })
            .await
            .context("backup restore task failed")
            .flatten();

            app.state::<RevisionSignal<AgentRevisionState>>()
                .notify(AgentRevisionState);
            result
        }
    };

    result.map_err(|error| {
        log::error!("direct change {name} failed: {error:#}");
        format!("{error:#}")
    })
}
