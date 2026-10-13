use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use std::{
    io::{self, IsTerminal as _, Write as _},
    process::ExitCode,
};
use wizard_core::{
    agents::{
        self,
        claude::{self, ProxyMode as ClaudeMode},
        codex::{self, ProxyMode as CodexMode},
        opencode, pi,
    },
    prepared_plan::PreparedPlan,
    settings, validate_token,
};

#[derive(Parser)]
#[command(
    version,
    about = "CoreInfra Wizard backend CLI. Close the GUI before changing settings."
)]
struct Cli {
    /// Apply the displayed plan without an interactive confirmation.
    #[arg(long, global = true)]
    yes: bool,
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
            settings::update_file(|settings| state.update_settings_paths(settings))?;
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        Command::SetToken => {
            let mut token = String::new();
            io::stdin().read_line(&mut token)?;
            let token = token.trim().to_owned();
            validate_token(&token)?;

            settings::update_file(|settings| settings.coreinfra_token = token)?;
        }
        Command::Codex { command } => {
            let settings = settings::load_from_file()?;

            run_plan(&runtime, cli.yes, move || {
                let mode = match command {
                    ProxyCommand::Hub => CodexMode::ProxyHub,
                    ProxyCommand::Api => CodexMode::ProxyApi,
                    ProxyCommand::None => CodexMode::Disabled,
                };
                codex::prepare_proxy(mode, &settings)
            })?;
        }
        Command::Claude { command } => {
            let settings = settings::load_from_file()?;

            run_plan(&runtime, cli.yes, move || {
                let mode = match command {
                    ProxyCommand::Hub => ClaudeMode::ProxyHub,
                    ProxyCommand::Api => ClaudeMode::ProxyApi,
                    ProxyCommand::None => ClaudeMode::Disabled,
                };
                claude::prepare_proxy(mode, &settings)
            })?;
        }
        Command::Pi { command } => {
            let settings = settings::load_from_file()?;
            let install = matches!(command, HubCommand::Hub);

            run_plan(&runtime, cli.yes, move || {
                pi::prepare_hub(install, &settings)
            })?;
        }
        Command::Opencode { command } => {
            let settings = settings::load_from_file()?;
            let install = matches!(command, HubCommand::Hub);

            run_plan(&runtime, cli.yes, move || {
                opencode::prepare_hub(install, &settings)
            })?;
        }
    }
    Ok(())
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
