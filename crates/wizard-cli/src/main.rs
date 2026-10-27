mod account;
mod agent;

use account::AccountCommand;
use agent::AgentCommand;
use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::process::ExitCode;
use wizard_core::hub;

#[derive(Parser)]
#[command(
    version,
    about = "CoreInfra Wizard backend CLI. Close the GUI before running.",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show Wizard's configuration and log directories.
    Paths,
    /// Manage the `CoreInfra` Hub account.
    #[command(disable_help_subcommand = true)]
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Configure agents and the token they use.
    #[command(disable_help_subcommand = true)]
    Agent {
        /// Confirm without prompting: creating a Hub token and applying the displayed plan.
        #[arg(long, global = true)]
        yes: bool,
        #[command(subcommand)]
        command: AgentCommand,
    },
}

fn main() -> ExitCode {
    if let Some(arg1) = std::env::args_os().nth(1)
        && arg1 == "--proxy"
    {
        return wizard_proxy_daemon::main();
    }

    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("wizard-cli: {error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(cli: Cli) -> Result<()> {
    hub::install_crypto_provider()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create CLI runtime")?;
    match cli.command {
        Command::Paths => {
            let paths = serde_json::json!({
                "config_dir": wizard_settings::paths::config_dir()?,
                "logs_dir": wizard_settings::paths::logs_dir()?,
            });
            println!("{}", serde_json::to_string_pretty(&paths)?);
            Ok(())
        }
        Command::Account { command } => account::run(&runtime, &command),
        Command::Agent { yes, command } => agent::run(&runtime, yes, command),
    }
}

#[test]
fn verify_cli() {
    use clap::CommandFactory as _;
    Cli::command().debug_assert();
}
