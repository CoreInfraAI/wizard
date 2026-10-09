use crate::utils::{Paths, clean_build_command, paths, require_success, stable_version};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{path::PathBuf, process::Command};

mod dev;
mod dev_tag;
mod release;
mod utils;

const DEV_ENDPOINT: &str = "https://coreinfraai.github.io/wizard/latest-dev.json";
const DEV_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY5QkI4Q0FEN0QxNTZDNkIKUldScmJCVjlyWXk3K2Q1VERmZ01oMzdVVDZPdG41VExtdkI5N3pHbXZYc3dxSEtPT0dHYTRaeksK";

#[derive(Parser)]
/// The task runner
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run with the Tauri CLI and the dev updater channel.
    Run {
        /// Use the standard Cargo release profile.
        #[arg(long)]
        release: bool,
    },
    /// Launch the installed application, installing it first if missing.
    DevApp {
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
    /// Check frontend types and run workspace formatting, compilation, lints, and tests.
    Ci,
    /// Find or create a GitHub draft release.
    CreateRelease {
        /// Create a development prerelease for the current workflow run.
        #[arg(long)]
        dev: bool,
    },
    /// Build, rename, and stage release artifacts.
    ReleaseBuild {
        /// Build a development prerelease for the current workflow run.
        #[arg(long)]
        dev: bool,
        /// Version produced by `create-release --dev`.
        #[arg(long, requires = "dev")]
        version: Option<String>,
        /// Rust target triple to build.
        #[arg(long)]
        target: String,
        /// Directory in which to stage files for upload.
        #[arg(long)]
        output: PathBuf,
    },
    /// Generate and upload Tauri updater metadata for a GitHub draft release.
    LatestJson {
        /// Generate metadata for a development prerelease.
        #[arg(long)]
        dev: bool,
        /// GitHub repository in `owner/name` format.
        #[arg(long)]
        repository: String,
        /// Numeric ID of the GitHub draft release containing all updater assets.
        #[arg(long)]
        release_id: u64,
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
        Commands::Run { release } | Commands::DevApp { release, .. } => *release,
        Commands::Ci
        | Commands::CreateRelease { .. }
        | Commands::ReleaseBuild { .. }
        | Commands::LatestJson { .. }
        | Commands::Version { .. } => false,
    };
    let paths = paths(release)?;

    match command {
        Commands::Run { release } => dev::run(&paths, release),
        Commands::DevApp {
            reinstall,
            release,
            console,
        } => dev::dev_app(&paths, reinstall, console, release),
        Commands::Ci => run_ci(&paths),
        Commands::CreateRelease { dev } => release::create(&paths, dev),
        Commands::ReleaseBuild {
            dev,
            version,
            target,
            output,
        } => release::build(&paths, dev, version.as_deref(), &target, &output),
        Commands::LatestJson {
            dev,
            repository,
            release_id,
        } => release::generate_latest_json(&paths, dev, &repository, release_id),
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
    let status = Command::new("node")
        .args(["node_modules/typescript/bin/tsc", "--noEmit"])
        .current_dir(paths.wizard.join("wizard-ui"))
        .status()?;
    require_success(
        status,
        "frontend TypeScript check (run npm ci in wizard-ui first)",
    )?;

    let status = clean_build_command("cargo")
        .args(["fmt", "--check"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo fmt --check")?;

    let status = clean_build_command("cargo")
        .args(["check", "--workspace", "--all-targets"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo check")?;

    let status = clean_build_command("cargo")
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

    let status = clean_build_command("cargo")
        .args(["test", "--workspace"])
        .current_dir(&paths.workspace)
        .status()?;
    require_success(status, "cargo test")
}
