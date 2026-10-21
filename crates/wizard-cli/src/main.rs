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
    about = "CoreInfra Wizard backend CLI. Close the GUI before changing settings.",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
        Command::Account { command } => account::run(&runtime, &command),
        Command::Agent { yes, command } => agent::run(&runtime, yes, command),
    }
}
