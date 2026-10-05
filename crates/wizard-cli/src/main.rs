use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::{io, process::ExitCode};
use wizard_core::{agents, settings, validate_token};

#[derive(Parser)]
#[command(
    version,
    about = "CoreInfra Wizard backend CLI. Close the GUI before changing settings."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print agent state as JSON (never includes the API token).
    Status,
    /// Read the token from stdin. Whitespace is trimmed; empty input clears it.
    SetToken,
    /// Manage the Codex proxy configuration.
    Codex {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// Manage the Claude Code proxy configuration.
    Claude {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    /// Manage the Pi Hub plugin.
    Pi {
        #[command(subcommand)]
        command: HubCommand,
    },
    /// Manage the `OpenCode` Hub plugin.
    Opencode {
        #[command(subcommand)]
        command: HubCommand,
    },
}

#[derive(Subcommand)]
enum ProxyCommand {
    /// Use `CoreInfra` Hub with the token saved in Wizard settings.
    Hub,
    /// Use `CoreInfra` API with existing subscription authentication and the saved token.
    Api,
    /// Remove the `CoreInfra` proxy configuration.
    None,
}

#[derive(Subcommand)]
enum HubCommand {
    /// Install the latest plugin and use the token saved in Wizard settings.
    Hub,
    /// Remove the plugin configuration and its saved token.
    None,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("wizard-cli: {error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(cli: Cli) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create CLI runtime")?;
    match cli.command {
        Command::Status => {
            let settings = settings::load_from_file()?;
            let state = runtime.block_on(agents::collect_agent_state(&settings));
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Command::SetToken => {
            let mut token = String::new();
            io::stdin().read_line(&mut token)?;
            let token = token.trim().to_owned();
            validate_token(&token)?;
            settings::update_file(|settings| {
                settings.coreinfra_api_key = token;
            })?;
        }
        Command::Codex { command } => {
            let mode = match command {
                ProxyCommand::Hub => agents::codex::ProxyMode::ProxyHub,
                ProxyCommand::Api => agents::codex::ProxyMode::ProxyApi,
                ProxyCommand::None => agents::codex::ProxyMode::Disabled,
            };
            let token = if mode == agents::codex::ProxyMode::Disabled {
                String::new()
            } else {
                settings::load_from_file()?.coreinfra_api_key
            };
            agents::codex::set_proxy(mode, &token)?;
        }
        Command::Claude { command } => {
            let mode = match command {
                ProxyCommand::Hub => agents::claude::ProxyMode::ProxyHub,
                ProxyCommand::Api => agents::claude::ProxyMode::ProxyApi,
                ProxyCommand::None => agents::claude::ProxyMode::Disabled,
            };
            let token = if mode == agents::claude::ProxyMode::Disabled {
                String::new()
            } else {
                settings::load_from_file()?.coreinfra_api_key
            };
            agents::claude::set_proxy(mode, &token)?;
        }
        Command::Pi { command } => run_hub_command(&runtime, &command, agents::pi::set_hub)?,
        Command::Opencode { command } => {
            run_hub_command(&runtime, &command, agents::opencode::set_hub)?;
        }
    }
    Ok(())
}

fn run_hub_command(
    runtime: &tokio::runtime::Runtime,
    command: &HubCommand,
    apply: fn(bool, &str) -> Result<()>,
) -> Result<()> {
    let install = matches!(command, HubCommand::Hub);
    let token = if install {
        settings::load_from_file()?.coreinfra_api_key
    } else {
        String::new()
    };
    runtime.block_on(async {
        tokio::task::spawn_blocking(move || apply(install, &token))
            .await
            .context("Hub command task failed")?
    })
}
