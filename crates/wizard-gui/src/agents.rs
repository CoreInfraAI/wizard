//! Tauri commands for agent discovery and configuration.

use crate::{hub_tokens, proxy, revision_signal::RevisionSignal, settings};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use wizard_core::agents::{
    AgentStates, Backup, BackupAgent, HubMode, RoutingMode, TokenAgent, claude, claude_desktop,
    codex, collect_agent_state, opencode, pi,
};
use wizard_core::prepared_plan::{PreparedPlan, PreparedStep, PreparedStepInfo};

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
pub(crate) async fn launch_agent(
    app: tauri::AppHandle,
    agent: wizard_core::agents::launch::LaunchAgent,
) -> Result<(), String> {
    ensure_agent_proxy(app.clone(), agent).await?;
    let settings = settings::get_state(&app).map_err(|error| format!("{error:#}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        wizard_core::agents::launch::launch(agent, &settings)
    })
    .await
    .context("Не удалось выполнить запуск агента")
    .flatten()
    .map_err(|error| format!("{error:#}"))
}

fn needs_proxy(
    agent: wizard_core::agents::launch::LaunchAgent,
    settings: &wizard_settings::Settings,
) -> bool {
    use wizard_core::agents::launch::LaunchAgent;
    match agent {
        LaunchAgent::ClaudeDesktop => settings.claude_proxy_enabled,
        LaunchAgent::ChatGpt => settings.codex_proxy_enabled,
        LaunchAgent::Claude | LaunchAgent::Codex | LaunchAgent::OpenCode | LaunchAgent::Pi => false,
    }
}

#[cfg(test)]
mod tests {
    use super::needs_proxy;
    use wizard_core::agents::launch::LaunchAgent;

    #[test]
    fn cli_launch_does_not_require_the_desktop_proxy() {
        for enabled in [false, true] {
            let settings = wizard_settings::Settings {
                claude_proxy_enabled: enabled,
                codex_proxy_enabled: enabled,
                ..Default::default()
            };
            assert_eq!(needs_proxy(LaunchAgent::ChatGpt, &settings), enabled);
            assert_eq!(needs_proxy(LaunchAgent::ClaudeDesktop, &settings), enabled);
            for cli in [
                LaunchAgent::Codex,
                LaunchAgent::Claude,
                LaunchAgent::OpenCode,
                LaunchAgent::Pi,
            ] {
                assert!(!needs_proxy(cli, &settings));
            }
        }
    }
}

/// Opening an agent restores only its saved proxy configuration, never its routing mode.
#[tauri::command]
pub(crate) async fn ensure_agent_proxy(
    app: tauri::AppHandle,
    agent: wizard_core::agents::launch::LaunchAgent,
) -> Result<(), String> {
    let settings =
        settings::get_state(&app).map_err(|_| "Не удалось прочитать настройки Wizard")?;
    if !needs_proxy(agent, &settings) {
        return Ok(());
    }
    proxy::observe(&app).await;
    if proxy::running(&app) {
        return Ok(());
    }
    let result = async {
        proxy::prepare_token(&app).await?;
        proxy::sync(&app).await?;
        let settings = settings::get_state(&app)?;
        anyhow::ensure!(
            !needs_proxy(agent, &settings) || proxy::running(&app),
            "Локальный прокси не подтвердил готовность"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    result.map_err(|error| {
        log::error!("proxy startup on agent open failed: {error:#}");
        format!("Не удалось запустить локальный прокси: {error:#}")
    })
}

#[tauri::command]
pub(crate) async fn get_agent_backups(agent: BackupAgent) -> Result<Vec<Backup>, String> {
    // TODO: files can make this response large and include credentials.
    // Consider loading file contents on demand if backup inspection is added to the UI.
    // Never log the response.
    tauri::async_runtime::spawn_blocking(move || match agent {
        BackupAgent::Codex => codex::get_backups(),
        BackupAgent::Claude => claude::get_backups(),
        BackupAgent::ClaudeDesktop => claude_desktop::get_backups(),
    })
    .await
    .context("backup listing task failed")
    .flatten()
    .map_err(|error| format!("{error:#}"))
}

/// Use an existing value or provision the agent's default Hub token for an enabled mode.
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
    ClaudeDesktopSetMedia(bool),
    SetPiHub(HubMode),
    SetOpenCodeHub(HubMode),
}

// No Debug: the plan contains credentials; only info is intended for display.
#[derive(Serialize)]
pub(crate) struct PreparedAgentChange {
    change: AgentChange,
    agent: &'static str,
    // Resolved credential for explicit re-preparation; never repeat TokenChoice::Create.
    token: Option<String>,
    plan: PreparedPlan,
    info: Vec<PreparedStepInfo>,
    restarts_desktop: bool,
}

impl AgentChange {
    const fn needs_agent_token(self) -> bool {
        match self {
            Self::CodexSetRouting(mode) | Self::ClaudeSetRouting(mode) => {
                !matches!(mode, RoutingMode::Disabled)
            }
            Self::ClaudeDesktopSetRouting(mode) => matches!(mode, RoutingMode::Hub),
            Self::ClaudeDesktopSetMedia(_) => false,
            Self::SetPiHub(mode) | Self::SetOpenCodeHub(mode) => !matches!(mode, HubMode::Disabled),
        }
    }

    const fn token_agent(self) -> TokenAgent {
        match self {
            Self::CodexSetRouting(_) => TokenAgent::Codex,
            Self::ClaudeSetRouting(_) => TokenAgent::Claude,
            Self::ClaudeDesktopSetRouting(_) | Self::ClaudeDesktopSetMedia(_) => {
                TokenAgent::ClaudeDesktop
            }
            Self::SetPiHub(_) => TokenAgent::Pi,
            Self::SetOpenCodeHub(_) => TokenAgent::OpenCode,
        }
    }
}

/// Enabled modes require `token`, except Claude Desktop subscription which uses
/// Wizard's shared proxy credential. Never log tokens. Newly issued defaults remain
/// in the Hub if preparation fails or is cancelled.
#[tauri::command]
pub(crate) async fn prepare_agent_change(
    change: AgentChange,
    token: Option<TokenChoice>,
    // The draft's media choice, applied right after this plan; otherwise the saved grant.
    media: Option<bool>,
    app: tauri::AppHandle,
) -> Result<PreparedAgentChange, String> {
    let (name, agent) = match &change {
        AgentChange::CodexSetRouting(_) => ("CodexSetRouting", "codex"),
        AgentChange::ClaudeSetRouting(_) => ("ClaudeSetRouting", "claude"),
        AgentChange::ClaudeDesktopSetRouting(_) => ("ClaudeDesktopSetRouting", "claude_desktop"),
        AgentChange::ClaudeDesktopSetMedia(_) => ("ClaudeDesktopSetMedia", "claude_desktop"),
        AgentChange::SetPiHub(_) => ("SetPiHub", "pi"),
        AgentChange::SetOpenCodeHub(_) => ("SetOpenCodeHub", "opencode"),
    };
    log::info!("received agent change: {name}");

    let settings = settings::get_state(&app).map_err(|error| format!("{error:#}"))?;

    let token = if change.needs_agent_token() {
        match token {
            Some(TokenChoice::Existing { token }) if !token.is_empty() => token,
            Some(TokenChoice::Create) if matches!(change, AgentChange::CodexSetRouting(_)) => {
                hub_tokens::codex_token(&app).await?
            }
            Some(TokenChoice::Create) => {
                hub_tokens::issue(&app, change.token_agent().token_name()).await?
            }
            _ => return Err("Выберите токен CoreInfra Hub".into()),
        }
    } else {
        String::new()
    };

    let reprepare_token = (!token.is_empty()).then(|| token.clone());

    // Fetch before taking the file lock or preparing changes. A failed fetch leaves the profile intact.
    let desktop_models = if matches!(
        change,
        AgentChange::ClaudeDesktopSetRouting(RoutingMode::Hub)
    ) {
        Some(
            claude_desktop::Models::fetch(&token)
                .await
                .map_err(|error| format!("{error:#}"))?,
        )
    } else {
        None
    };

    // Plans contain credentials: never log the returned value.
    let plan = tauri::async_runtime::spawn_blocking(move || match change {
        AgentChange::CodexSetRouting(mode) => {
            codex::prepare_routing_for_gui(mode, &token, &settings)
        }
        AgentChange::ClaudeSetRouting(mode) => claude::prepare_routing(
            mode,
            &token,
            media.unwrap_or(settings.media.claude.is_some()),
        ),
        AgentChange::ClaudeDesktopSetRouting(mode) => {
            let media = media.unwrap_or(settings.media.claude_desktop.is_some());
            let mut plan =
                claude_desktop::prepare_routing(mode, &token, media, desktop_models.as_ref())?;
            plan.restart_desktop(
                wizard_core::agents::desktop_install::DesktopApp::Claude,
                settings.last_paths.claude_desktop.as_deref(),
            );
            Ok(plan)
        }
        AgentChange::ClaudeDesktopSetMedia(enabled) => {
            let mut plan = claude_desktop::prepare_media(enabled)?;
            if !plan.info().is_empty() {
                plan.restart_desktop(
                    wizard_core::agents::desktop_install::DesktopApp::Claude,
                    settings.last_paths.claude_desktop.as_deref(),
                );
            }
            Ok(plan)
        }
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
    let restarts_desktop = plan.restarts_desktop();

    log::info!("agent change prepared: {name}");
    Ok(PreparedAgentChange {
        change,
        agent,
        token: reprepare_token,
        plan,
        info,
        restarts_desktop,
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

    let enables_proxy = plan.steps().iter().any(|step| {
        matches!(
            step,
            PreparedStep::SetClaudeProxyEnabled { enabled: true }
                | PreparedStep::SetCodexProxyEnabled { enabled: true }
        )
    });
    // Do not touch agent files if the Wizard token cannot be obtained.
    let wizard_token = if enables_proxy {
        Some(hub_tokens::proxy_token(&app).await?)
    } else {
        None
    };

    let proxy_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        // Keep account/Hub changes out from preflight through the last file/proxy step.
        let guard = settings::lock_state()?;
        if let Some(token) = &wizard_token {
            anyhow::ensure!(
                settings::get_state(&proxy_app)?.hub_session == token.session,
                "Сессия CoreInfra Hub изменилась; повторите применение"
            );
        }

        // Sync even when the setting is unchanged: «Применить» starts a stopped daemon.
        // Reload updates the shared slot; an unchanged token preserves active connections.
        // Even if saving fails, synchronize with the settings actually on disk.
        plan.apply_with_proxies(
            |enabled| {
                let saved = settings::update_state_locked(&proxy_app, &guard, |settings| {
                    if let Some(token) = wizard_token.as_ref().filter(|_| enabled) {
                        settings.proxy_token = Some(token.value.clone());
                    }
                    settings.claude_proxy_enabled = enabled;
                });
                let synced = tauri::async_runtime::block_on(proxy::sync(&proxy_app));
                saved.and(synced)
            },
            |enabled| {
                let saved = settings::update_state_locked(&proxy_app, &guard, |settings| {
                    if let Some(token) = wizard_token.as_ref().filter(|_| enabled) {
                        settings.proxy_token = Some(token.value.clone());
                    }
                    settings.codex_proxy_enabled = enabled;
                });
                let synced = tauri::async_runtime::block_on(proxy::sync(&proxy_app));
                saved.and(synced)
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

    if settings::get_state(&app)
        .map_err(|e| format!("{e:#}"))?
        .media
        .has_grants()
    {
        proxy::sync(&app).await.map_err(|e| format!("{e:#}"))?;
    }
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
        DirectChange::RestoreBackup {
            agent: BackupAgent::ClaudeDesktop,
            id,
        } => {
            let media = settings::get_state(&app)
                .map_err(|error| format!("{error:#}"))?
                .media
                .claude_desktop
                .is_some();
            let plan = tauri::async_runtime::spawn_blocking(move || {
                claude_desktop::prepare_restore_backup(id, media)
            })
            .await
            .context("backup restore preparation task failed")
            .flatten()
            .map_err(|error| format!("{error:#}"))?;
            return apply_agent_plan(plan, app).await;
        }
        DirectChange::RestoreBackup { agent, id } => {
            let restore_app = app.clone();
            let result = tauri::async_runtime::spawn_blocking(move || match agent {
                BackupAgent::Codex => {
                    let _guard = settings::lock_state()?;
                    codex::restore_backup(id, &settings::get_state(&restore_app)?)
                }
                BackupAgent::Claude => {
                    let media = settings::get_state(&restore_app)?.media.claude.is_some();
                    claude::restore_backup(id, media)
                }
                BackupAgent::ClaudeDesktop => unreachable!(),
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
    })?;
    if settings::get_state(&app)
        .map_err(|e| format!("{e:#}"))?
        .media
        .has_grants()
    {
        proxy::sync(&app).await.map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}
