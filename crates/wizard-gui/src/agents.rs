//! Tauri commands for agent discovery and configuration.

use crate::{hub_tokens, proxy, revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use wizard_core::agents::{
    AgentStates, Backup, BackupAgent, HubMode, RoutingMode, TokenAgent, claude, claude_desktop,
    codex, collect_agent_state, opencode, pi,
};
use wizard_core::prepared_plan::{PreparedPlan, PreparedStepInfo};

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

    // Only a cache for the next detection: never fail the state because it was not saved.
    let detected = state.clone();
    if let Err(error) = settings::update_state(&app, move |settings| {
        detected.update_settings_paths(settings);
    })
    .await
    {
        log::error!("failed to save discovered agent paths: {error:#}");
    }

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

/// Token for an enabled mode: an existing value, or a new Hub token named after the agent.
// No Debug: the value is a credential.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TokenChoice {
    Existing { token: String },
    Create,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum AgentChange {
    CodexSetRouting(RoutingMode),
    ClaudeSetRouting(RoutingMode),
    ClaudeDesktopSetRouting(RoutingMode),
    SetPiHub(HubMode),
    SetOpenCodeHub(HubMode),
}

// No Debug: the plan contains credentials; only info is intended for display.
#[derive(Serialize)]
pub(crate) struct PreparedAgentChange {
    change: AgentChange,
    agent: &'static str,
    plan: PreparedPlan,
    info: Vec<PreparedStepInfo>,
}

impl AgentChange {
    const fn enabled(self) -> bool {
        match self {
            Self::CodexSetRouting(mode)
            | Self::ClaudeSetRouting(mode)
            | Self::ClaudeDesktopSetRouting(mode) => !matches!(mode, RoutingMode::Disabled),
            Self::SetPiHub(mode) | Self::SetOpenCodeHub(mode) => !matches!(mode, HubMode::Disabled),
        }
    }

    const fn token_agent(self) -> TokenAgent {
        match self {
            Self::CodexSetRouting(_) => TokenAgent::Codex,
            Self::ClaudeSetRouting(_) => TokenAgent::Claude,
            Self::ClaudeDesktopSetRouting(_) => TokenAgent::ClaudeDesktop,
            Self::SetPiHub(_) => TokenAgent::Pi,
            Self::SetOpenCodeHub(_) => TokenAgent::OpenCode,
        }
    }
}

/// Enabled modes require `token`; it is not validated. Never log it. `Create` issues the Hub
/// token before preparation and is not part of the plan: cancelling or a failed preparation
/// keeps the token, already published in the Hub token list.
#[tauri::command]
pub(crate) async fn prepare_agent_change(
    change: AgentChange,
    token: Option<TokenChoice>,
    app: tauri::AppHandle,
) -> Result<PreparedAgentChange, String> {
    let (name, agent) = match &change {
        AgentChange::CodexSetRouting(_) => ("CodexSetRouting", "codex"),
        AgentChange::ClaudeSetRouting(_) => ("ClaudeSetRouting", "claude"),
        AgentChange::ClaudeDesktopSetRouting(_) => ("ClaudeDesktopSetRouting", "claude_desktop"),
        AgentChange::SetPiHub(_) => ("SetPiHub", "pi"),
        AgentChange::SetOpenCodeHub(_) => ("SetOpenCodeHub", "opencode"),
    };
    log::info!("received agent change: {name}");

    let settings = settings::get_state(&app).map_err(|error| format!("{error:#}"))?;

    let token = if change.enabled() {
        match token {
            Some(TokenChoice::Existing { token }) if !token.is_empty() => token,
            Some(TokenChoice::Create) => {
                hub_tokens::issue(&app, change.token_agent().token_name()).await?
            }
            _ => return Err("Выберите токен CoreInfra Hub".into()),
        }
    } else {
        String::new()
    };

    // Plans contain credentials: never log the returned value.
    let plan = tauri::async_runtime::spawn_blocking(move || match change {
        AgentChange::CodexSetRouting(mode) => {
            codex::prepare_routing_for_gui(mode, &token, &settings)
        }
        AgentChange::ClaudeSetRouting(mode) => claude::prepare_routing(mode, &token),
        AgentChange::ClaudeDesktopSetRouting(mode) => claude_desktop::prepare_routing(mode, &token),
        AgentChange::SetPiHub(mode) => pi::prepare_hub(mode, &token, &settings),
        AgentChange::SetOpenCodeHub(mode) => opencode::prepare_hub(mode, &token, &settings),
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
        // Sync even when the setting is unchanged: «Применить» restarts a stopped listener.
        // Also sync after a failed save: tray or autostart can fail after the setting is saved.
        plan.apply_with_proxies(
            |token| {
                tauri::async_runtime::block_on(async {
                    let saved = settings::update_state(&proxy_app, move |settings| {
                        settings.claude_proxy_enabled = token;
                    })
                    .await;
                    let synced = proxy::sync_one(&proxy_app, proxy::App::ClaudeDesktop).await;
                    saved.and(synced)
                })
            },
            |token| {
                tauri::async_runtime::block_on(async {
                    let saved = settings::update_state(&proxy_app, move |settings| {
                        settings.codex_proxy_enabled = token;
                    })
                    .await;
                    let synced = proxy::sync_one(&proxy_app, proxy::App::ChatGpt).await;
                    saved.and(synced)
                })
            },
        )
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

#[derive(Deserialize)]
pub(crate) enum DirectChange {
    RestoreBackup { agent: BackupAgent, id: u32 },
}

#[tauri::command]
pub(crate) async fn apply_direct_change(
    change: DirectChange,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let name = match &change {
        DirectChange::RestoreBackup { .. } => "RestoreBackup",
    };
    log::info!("received direct change: {name}");

    let result = match change {
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
