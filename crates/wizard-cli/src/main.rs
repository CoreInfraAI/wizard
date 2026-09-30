use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::{io, process::ExitCode};
use wizard_core::{agents, settings};

#[derive(Parser)]
#[command(
    version,
    about = "Coreinfra Wizard backend CLI. Close the GUI before changing settings."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print agent state as JSON (never includes the API token).
    Status,
    /// Manage Wizard settings on disk.
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Manage the Codex proxy configuration.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },
}

#[derive(Subcommand)]
enum SettingsCommand {
    /// Read the token from stdin. Whitespace is trimmed; empty input clears it.
    SetToken,
}

#[derive(Subcommand)]
enum CodexCommand {
    /// Configure or disable the Codex proxy.
    Proxy {
        #[command(subcommand)]
        command: CodexProxyCommand,
    },
}

#[derive(Subcommand)]
enum CodexProxyCommand {
    /// Use `CoreInfra` Hub with the token saved in Wizard settings.
    Hub,
    /// Use `CoreInfra` API with `ChatGPT` authentication and the saved token.
    Api,
    /// Remove the `CoreInfra` proxy configuration.
    Unset,
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
    match cli.command {
        Command::Status => {
            let settings = settings::load_from_file()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("failed to create CLI runtime")?;
            let state = runtime.block_on(agents::collect_agent_state(&settings));
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Command::Settings {
            command: SettingsCommand::SetToken,
        } => {
            let mut token = String::new();
            io::stdin().read_line(&mut token)?;
            let token = token.trim().to_owned();
            settings::update_file(|settings| {
                settings.coreinfra_api_key = token;
            })?;
        }
        Command::Codex {
            command: CodexCommand::Proxy { command },
        } => {
            let mode = match command {
                CodexProxyCommand::Hub => agents::codex::ProxyMode::ProxyHub,
                CodexProxyCommand::Api => agents::codex::ProxyMode::ProxyApi,
                CodexProxyCommand::Unset => agents::codex::ProxyMode::Disabled,
            };
            let token = if mode == agents::codex::ProxyMode::Disabled {
                String::new()
            } else {
                settings::load_from_file()?.coreinfra_api_key
            };
            agents::codex::set_proxy(mode, &token)?;
        }
    }
    Ok(())
}
