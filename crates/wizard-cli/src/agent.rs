//! `agent`: agent state, the saved token and routing plans.

use anyhow::{Context as _, Result, ensure};
use clap::Subcommand;
use std::io::{self, IsTerminal as _, Write as _};
use wizard_core::{
    agents::{self, HubMode, RoutingMode, claude, codex, opencode, pi},
    prepared_plan::PreparedPlan,
    settings, validate_token,
};

#[derive(Subcommand)]
pub(crate) enum AgentCommand {
    /// Print agent state as JSON (never includes the API token).
    Status,
    /// Read the token from stdin. Whitespace is trimmed; empty input clears it.
    SetToken,
    /// Manage the Codex routing configuration.
    #[command(disable_help_subcommand = true)]
    Codex {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Manage the Claude Code routing configuration.
    #[command(disable_help_subcommand = true)]
    Claude {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Claude Desktop configuration is available only in the GUI.
    #[command(disable_help_subcommand = true)]
    ClaudeDesktop {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Manage the Pi Hub plugin.
    #[command(disable_help_subcommand = true)]
    Pi {
        #[command(subcommand)]
        command: HubCommand,
    },
    /// Manage the `OpenCode` Hub plugin.
    #[command(disable_help_subcommand = true)]
    Opencode {
        #[command(subcommand)]
        command: HubCommand,
    },
}

#[derive(Subcommand)]
pub(crate) enum RoutingCommand {
    /// Use `CoreInfra` Hub with the token saved in Wizard settings.
    Hub,
    /// Use `CoreInfra` Sub with existing subscription authentication and the saved token.
    Sub,
    /// Remove the `CoreInfra` routing configuration.
    None,
}

#[derive(Subcommand)]
pub(crate) enum HubCommand {
    /// Configure Hub with the saved token, installing a plugin when required.
    Hub,
    /// Remove the `CoreInfra` Hub configuration and its saved token.
    None,
}

pub(crate) fn run(
    runtime: &tokio::runtime::Runtime,
    yes: bool,
    command: AgentCommand,
) -> Result<()> {
    match command {
        AgentCommand::Status => {
            let settings = settings::load_from_file()?;
            let mut state = runtime.block_on(agents::collect_agent_state(&settings));
            settings::update_file(|settings| state.update_settings_paths(settings))?;
            state.redact_tokens();
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        AgentCommand::SetToken => {
            let mut token = String::new();
            io::stdin().read_line(&mut token)?;
            let token = token.trim().to_owned();
            validate_token(&token)?;

            settings::update_file(|settings| settings.coreinfra_token = token)?;
        }
        AgentCommand::Codex { command } => {
            let settings = settings::load_from_file()?;
            let mode = routing_mode(&command);

            run_plan(runtime, yes, move || {
                codex::prepare_routing(mode, &settings.coreinfra_token, &settings)
            })?;
        }
        AgentCommand::Claude { command } => {
            let settings = settings::load_from_file()?;
            let mode = routing_mode(&command);

            run_plan(runtime, yes, move || {
                claude::prepare_routing(mode, &settings.coreinfra_token)
            })?;
        }
        AgentCommand::ClaudeDesktop { .. } => {
            anyhow::bail!(
                "Настройка Claude Desktop доступна только в GUI: прокси-сервер запускается только в GUI."
            );
        }
        AgentCommand::Pi { command } => {
            let settings = settings::load_from_file()?;
            let mode = hub_mode(&command);

            run_plan(runtime, yes, move || {
                pi::prepare_hub(mode, &settings.coreinfra_token, &settings)
            })?;
        }
        AgentCommand::Opencode { command } => {
            let settings = settings::load_from_file()?;
            let mode = hub_mode(&command);

            run_plan(runtime, yes, move || {
                opencode::prepare_hub(mode, &settings.coreinfra_token, &settings)
            })?;
        }
    }
    Ok(())
}

const fn routing_mode(command: &RoutingCommand) -> RoutingMode {
    match command {
        RoutingCommand::Hub => RoutingMode::Hub,
        RoutingCommand::Sub => RoutingMode::Sub,
        RoutingCommand::None => RoutingMode::Disabled,
    }
}

const fn hub_mode(command: &HubCommand) -> HubMode {
    match command {
        HubCommand::Hub => HubMode::Hub,
        HubCommand::None => HubMode::Disabled,
    }
}

fn run_plan(
    runtime: &tokio::runtime::Runtime,
    yes: bool,
    prepare: impl FnOnce() -> Result<PreparedPlan> + Send + 'static,
) -> Result<()> {
    let plan = runtime.block_on(async {
        tokio::task::spawn_blocking(prepare)
            .await
            .context("plan preparation task failed")?
    })?;

    let preview = plan.info();
    if preview.is_empty() {
        eprintln!("Изменения не требуются.");
        return Ok(());
    }

    eprintln!("Wizard выполнит:");
    for (index, info) in preview.iter().enumerate() {
        eprintln!("{}. {}", index + 1, info.description);
    }
    eprintln!("Завершите текущую работу и закройте агента перед применением.");

    if !yes {
        ensure!(
            io::stdin().is_terminal(),
            "non-interactive execution requires --yes; nothing was applied"
        );

        eprint!("Применить? [y/N] ");
        io::stderr().flush()?;

        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;

        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("Отменено. Изменения не применены.");
            return Ok(());
        }
    }

    runtime.block_on(async {
        tokio::task::spawn_blocking(move || plan.apply())
            .await
            .context("plan application task failed")?
    })?;

    eprintln!("Готово.");
    Ok(())
}
