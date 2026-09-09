use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use semver::Version;
use serde_json::{Value, json};
use tempfile::TempDir;

const TEST_PRIVATE_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHJzaWduIGVuY3J5cHRlZCBzZWNyZXQga2V5ClJXUlRZMEl5TmlGT0xMc0FVYnN1aXZzWHZlWU9Ra3FzcFp3R1IwNGVDdk8rTTZsRTlTQUFBQkFBQUFBQUFBQUFBQUlBQUFBQUxqTG12cnlGellHNHRkNWo4ejdhRUt5YzdlUmZQZFg1dys2WE1QVHI5YWx6WnA5aTI2SlZrdWtwVDZ0emFCcTRnKy9DWFRZc2UvbGFHelVvaUY2dDNTNzJKanJZYVkrSjN4MTlQMHJXUUQxODg4ZTZWOS91Q0dCV1JDMlBVbHlIRmFuUnRBT3lVNzA9Cg==";
const TEST_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVFREVCMTI0NjlGQzY5QkUKUldTK2FmeHBKTEhlN3N6SWd1MG9QMU1XdmlydzhROEM3Q1dwdjhUbEFVbFBEV0hTczhKcCtBV3QK";
const TEST_ENDPOINT: &str = "http://127.0.0.1:49173/latest.json";
const APP_NAME: &str = "ff-wizard.app";
const INSTALLED_APP: &str = "/Applications/ff-wizard.app";
const INSTALLED_EXECUTABLE: &str = "/Applications/ff-wizard.app/Contents/MacOS/ff-wizard";

#[derive(Parser)]
/// The task runner
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run with `cargo tauri dev` and the test updater.
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
    Update {
        /// Use the standard Cargo release profile.
        #[arg(long)]
        release: bool,
    },
}

struct Paths {
    workspace: PathBuf,
    wizard: PathBuf,
    tauri_config: PathBuf,
    bundled_app: PathBuf,
    bundle_dir: PathBuf,
    installed_app: PathBuf,
    installed_executable: PathBuf,
}

fn main() -> Result<()> {
    let command = Cli::parse().command;
    let release = match &command {
        Commands::Run { release }
        | Commands::App { release, .. }
        | Commands::Update { release } => *release,
    };
    let paths = paths(release)?;

    match command {
        Commands::Run { release } => run_dev(&paths, release),
        Commands::App {
            reinstall,
            release,
            console,
        } => run_app(&paths, reinstall, console, release),
        Commands::Update { release } => update_server(&paths, release),
    }
}

/// Resolves workspace paths and selects the Cargo output directory for the requested profile.
fn paths(release: bool) -> Result<Paths> {
    let xtask = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = xtask
        .parent()
        .context("xtask must be inside the workspace")?
        .to_path_buf();
    let wizard = workspace.join("crates/wizard");
    let profile = if release { "release" } else { "debug" };
    let bundle_dir = workspace.join("target").join(profile).join("bundle");

    Ok(Paths {
        tauri_config: wizard.join("tauri.conf.json"),
        bundled_app: bundle_dir.join("macos").join(APP_NAME),
        installed_app: PathBuf::from(INSTALLED_APP),
        installed_executable: PathBuf::from(INSTALLED_EXECUTABLE),
        workspace,
        wizard,
        bundle_dir,
    })
}

/// Creates a temporary directory for generated Tauri configuration files.
fn temporary_configs() -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix("ff-wizard-tauri.")
        .tempdir_in("/tmp")
        .context("failed to create temporary Tauri config directory")
}

/// Writes a temporary Tauri config with the requested version and local test updater.
fn generate_config(
    paths: &Paths,
    output: &Path,
    version: &Version,
    create_updater_artifacts: bool,
) -> Result<()> {
    let mut config: Value = serde_json::from_slice(
        &fs::read(&paths.tauri_config)
            .with_context(|| format!("failed to read {}", paths.tauri_config.display()))?,
    )?;
    config["version"] = json!(version.to_string());
    config["bundle"]["createUpdaterArtifacts"] = json!(create_updater_artifacts);
    config["plugins"]["updater"] = json!({
        "endpoints": [TEST_ENDPOINT],
        "pubkey": TEST_PUBLIC_KEY,
        "dangerousInsecureTransportProtocol": true,
    });
    fs::write(output, serde_json::to_vec_pretty(&config)?)
        .with_context(|| format!("failed to write {}", output.display()))
}

/// Reads the stable base version from `tauri.conf.json` and rejects pre-release versions.
fn stable_version(paths: &Paths) -> Result<Version> {
    let config: Value = serde_json::from_slice(&fs::read(&paths.tauri_config)?)?;
    let version = config["version"]
        .as_str()
        .context("tauri.conf.json version must be a string")?;
    let version = Version::parse(version)?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        bail!("tauri.conf.json must contain a stable version, got {version}");
    }
    Ok(version)
}

/// Reads the currently installed application's version from its macOS property list.
fn installed_version(paths: &Paths) -> Option<Version> {
    let plist = paths.installed_app.join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(plist)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Version::parse(String::from_utf8_lossy(&output.stdout).trim()).ok()
}

/// Calculates the next `patch-dev.N` version relative to the stable and installed versions.
fn next_update_version(paths: &Paths) -> Result<Version> {
    let stable = stable_version(paths)?;
    let target_patch = stable.patch + 1;
    let mut sequence = 0_u64;

    if let Some(candidate) = installed_version(paths)
        && candidate.major == stable.major
        && candidate.minor == stable.minor
        && candidate.patch == target_patch
        && let Some(value) = candidate.pre.as_str().strip_prefix("dev.")
        && let Ok(value) = value.parse::<u64>()
    {
        sequence = value;
    }

    Version::parse(&format!(
        "{}.{}.{}-dev.{}",
        stable.major,
        stable.minor,
        target_patch,
        sequence + 1
    ))
    .context("failed to construct update version")
}

/// Runs the application through Tauri with the local test updater configured.
fn run_dev(paths: &Paths, release: bool) -> Result<()> {
    let configs = temporary_configs()?;
    let config = configs.path().join("tauri-dev.conf.json");
    generate_config(paths, &config, &stable_version(paths)?, false)?;

    let mut command = Command::new("cargo");
    command.arg("tauri").arg("dev");
    if release {
        command.arg("--release");
    }
    command
        .arg("--config")
        .arg(&config)
        .current_dir(&paths.wizard);
    require_success(command.status()?, "cargo tauri dev")
}

/// Installs the application when needed and launches it directly or via `LaunchServices`.
fn run_app(paths: &Paths, reinstall: bool, console: bool, release: bool) -> Result<()> {
    if reinstall || !paths.installed_executable.is_file() {
        let configs = temporary_configs()?;
        let config = configs.path().join("tauri-dev.conf.json");
        generate_config(paths, &config, &stable_version(paths)?, false)?;
        build_app(paths, &config, false, release)?;
        install_app(paths)?;
    }

    let mut command = if console {
        Command::new(&paths.installed_executable)
    } else {
        let mut command = Command::new("open");
        command.arg("-n").arg(&paths.installed_app);
        command
    };
    require_success(command.status()?, "failed to launch application")
}

/// Builds the macOS app bundle, optionally signing updater artifacts with the test key.
fn build_app(paths: &Paths, config: &Path, sign: bool, release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.arg("tauri").arg("build");
    if !release {
        command.arg("--debug");
    }
    command
        .args(["--bundles", "app", "--config"])
        .arg(config)
        .current_dir(&paths.wizard);
    if sign {
        command
            .env("TAURI_SIGNING_PRIVATE_KEY", TEST_PRIVATE_KEY)
            .env("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "");
    }
    require_success(command.status()?, "cargo tauri build")
}

/// Atomically replaces the application installed in `/Applications` with the built bundle.
fn install_app(paths: &Paths) -> Result<()> {
    let temporary_app = PathBuf::from(format!(
        "/Applications/.ff-wizard.app.tmp.{}",
        std::process::id()
    ));
    remove_path(&temporary_app)?;

    let status = Command::new("ditto")
        .arg(&paths.bundled_app)
        .arg(&temporary_app)
        .status()
        .context("failed to run ditto")?;
    if !status.success() {
        remove_path(&temporary_app)?;
        bail!("failed to copy application into /Applications");
    }

    let result = (|| {
        remove_path(&paths.installed_app)?;
        fs::rename(&temporary_app, &paths.installed_app)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = remove_path(&temporary_app);
    }
    result.context("failed to install /Applications/ff-wizard.app")
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Builds a signed development update and serves its metadata and archive over HTTP.
fn update_server(paths: &Paths, release: bool) -> Result<()> {
    let version = next_update_version(paths)?;
    let temporary = temporary_configs()?;
    let config = temporary.path().join("tauri-update.conf.json");
    let serve_dir = temporary.path().join("server");
    generate_config(paths, &config, &version, true)?;

    fs::create_dir(&serve_dir)?;
    remove_old_updater_archives(&paths.bundle_dir)?;

    println!("Building ff-wizard {version}...");
    build_app(paths, &config, true, release)?;
    let archive = find_update_archive(&paths.bundle_dir)?;
    write_latest_json(&serve_dir, &version, &archive)?;

    println!(
        "\nLocal update {version} is ready.\nEndpoint:  {TEST_ENDPOINT}\nArtifacts: {}\nStop the server with Ctrl-C.\n",
        serve_dir.display()
    );

    let mut command = Command::new("python3");
    command
        .args([
            "-m",
            "http.server",
            "49173",
            "--bind",
            "127.0.0.1",
            "--directory",
        ])
        .arg(&serve_dir)
        .current_dir(&paths.workspace);
    require_success(command.status()?, "local update server")
}

fn remove_old_updater_archives(bundle_dir: &Path) -> Result<()> {
    if !bundle_dir.exists() {
        return Ok(());
    }
    for path in files_recursively(bundle_dir)? {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.ends_with(".app.tar.gz") || name.ends_with(".app.tar.gz.sig") {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Finds the sole updater archive produced by the Tauri build.
fn find_update_archive(bundle_dir: &Path) -> Result<PathBuf> {
    let matches: Vec<_> = files_recursively(bundle_dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".app.tar.gz"))
        })
        .collect();
    if matches.len() != 1 {
        bail!(
            "expected exactly one updater archive, found {}",
            matches.len()
        );
    }
    Ok(matches[0].clone())
}

fn files_recursively(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
}

/// Copies updater artifacts into the server directory and writes `latest.json`.
fn write_latest_json(serve_dir: &Path, version: &Version, archive: &Path) -> Result<()> {
    let signature_path = PathBuf::from(format!("{}.sig", archive.display()));
    if !signature_path.is_file() {
        bail!(
            "updater signature was not generated: {}",
            signature_path.display()
        );
    }
    let artifact_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .context("updater archive has an invalid filename")?;
    fs::copy(archive, serve_dir.join(artifact_name))?;
    fs::copy(
        &signature_path,
        serve_dir.join(format!("{artifact_name}.sig")),
    )?;
    let latest = json!({
        "version": version.to_string(),
        "notes": "Local ff-wizard updater test",
        "url": format!("http://127.0.0.1:49173/{artifact_name}"),
        "signature": fs::read_to_string(signature_path)?.trim(),
    });
    fs::write(
        serve_dir.join("latest.json"),
        serde_json::to_vec_pretty(&latest)?,
    )?;
    Ok(())
}

fn require_success(status: ExitStatus, action: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("{action} exited with {status}")
    }
}
