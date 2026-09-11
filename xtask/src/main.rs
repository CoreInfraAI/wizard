use std::{path::PathBuf, process::Command};

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::utils::{Paths, paths, require_success, stable_version};

mod dev;
mod release;
mod utils;

#[derive(Parser)]
/// The task runner
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run with `cargo tauri dev` and the dev updater channel.
    Run {
        /// Use the standard Cargo release profile.
        #[arg(long)]
        release: bool,
    },
    /// Launch the installed application, installing it first if missing.
    App {
        /// Rebuild and replace the installed application.
        #[arg(long)]
        reinstall: bool,
        /// Use the standard Cargo release profile.
        #[arg(long)]
        release: bool,
        /// Run in the terminal and show application output.
        #[arg(long)]
        console: bool,
    },
    /// Build the next signed dev update and serve it locally.
    UpdateServer {
        /// Use the standard Cargo release profile.
        #[arg(long)]
        release: bool,
    },
    /// Run formatting, compilation, lint, and test checks for the workspace.
    Ci,
    /// Find or create a GitHub draft release.
    CreateRelease {
        /// Create a development prerelease for the current workflow run.
        #[arg(long)]
        dev: bool,
    },
    /// Build, rename, and stage production release artifacts.
    ReleaseBuild {
        /// Rust target triple to build.
        #[arg(long)]
        target: String,
        /// Directory in which to stage files for upload.
        #[arg(long)]
        output: PathBuf,
    },
    /// Generate and upload Tauri updater metadata for a GitHub draft release.
    LatestJson {
        /// GitHub repository in `owner/name` format.
        #[arg(long)]
        repository: String,
        /// GitHub release tag containing all updater assets.
        #[arg(long)]
        tag: String,
    },
    /// Print or check the current application version.
    Version {
        /// Require the application version to equal this value.
        #[arg(long)]
        check: Option<String>,
    },
}

fn main() -> Result<()> {
    let command = Cli::parse().command;
    let release = match &command {
        Commands::Run { release }
        | Commands::App { release, .. }
        | Commands::UpdateServer { release } => *release,
        Commands::Ci
        | Commands::CreateRelease { .. }
        | Commands::ReleaseBuild { .. }
        | Commands::LatestJson { .. }
        | Commands::Version { .. } => false,
    };
    let paths = paths(release)?;

    match command {
        Commands::Run { release } => dev::run(&paths, release),
        Commands::App {
            reinstall,
            release,
            console,
        } => dev::app(&paths, reinstall, console, release),
        Commands::UpdateServer { release } => dev::update_server(&paths, release),
        Commands::Ci => run_ci(&paths),
        Commands::CreateRelease { dev } => release::create(&paths, dev),
        Commands::ReleaseBuild { target, output } => release::build(&paths, &target, &output),
        Commands::LatestJson { repository, tag } => {
            release::generate_latest_json(&paths, &repository, &tag)
        }
        Commands::Version { check } => version(&paths, check.as_deref()),
    }
}

/// Prints the application version or checks it against an expected value.
fn version(paths: &Paths, expected: Option<&str>) -> Result<()> {
    let actual = stable_version(paths)?;
    if let Some(expected) = expected {
        if actual.to_string() != expected {
            anyhow::bail!("application version is {actual}, expected {expected}");
        }
    } else {
        println!("{actual}");
    }
    Ok(())
}

#[test]
fn verify_cli() {
    use clap::CommandFactory as _;
    Cli::command().debug_assert();
}

/// Runs the workspace checks used by continuous integration.
fn run_ci(paths: &Paths) -> Result<()> {
    let status = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo fmt --check")?;

    let status = Command::new("cargo")
        .args(["check", "--workspace", "--all-targets"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo check")?;

    let status = Command::new("cargo")
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo clippy")?;

    let status = Command::new("cargo")
        .args(["test", "--workspace"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo test")
}
