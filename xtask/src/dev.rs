use std::{fs, process::Command};

use anyhow::{Context as _, Result, bail};
use serde_json::json;
use crate::{DEV_ENDPOINT, DEV_PUBLIC_KEY};
use crate::utils::{Paths, clean_build_command, remove_path, require_success};

/// Runs the application through Tauri with the dev updater channel configured.
pub(crate) fn run(paths: &Paths, release: bool) -> Result<()> {
    let config = json!({
        "bundle": { "createUpdaterArtifacts": false },
        "plugins": {
            "updater": {
                "endpoints": [DEV_ENDPOINT],
                "pubkey": DEV_PUBLIC_KEY,
                "dangerousInsecureTransportProtocol": false,
            },
        },
    })
    .to_string();

    let mut command = clean_build_command("node");
    command.args(["wizard-ui/node_modules/@tauri-apps/cli/tauri.js", "dev"]);
    if release {
        command.arg("--release");
    }
    command
        .args(["--config", &config])
        .current_dir(&paths.wizard);
    require_success(command.status()?, "tauri dev")
}

/// Installs the application when needed and launches it directly or via `LaunchServices`.
pub(crate) fn dev_app(paths: &Paths, reinstall: bool, console: bool, release: bool) -> Result<()> {
    if reinstall || !paths.installed_executable.is_file() {
        let config = json!({
            "bundle": { "createUpdaterArtifacts": false },
            "plugins": {
                "updater": {
                    "endpoints": [DEV_ENDPOINT],
                    "pubkey": DEV_PUBLIC_KEY,
                },
            },
        })
        .to_string();
        build_app(paths, &config, release)?;
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

/// Builds the app bundle.
fn build_app(paths: &Paths, config: &str, release: bool) -> Result<()> {
    let mut command = clean_build_command("node");
    command.args(["wizard-ui/node_modules/@tauri-apps/cli/tauri.js", "build"]);
    if !release {
        command.arg("--debug");
    }
    command
        .args(["--bundles", "app", "--config", config])
        .current_dir(&paths.wizard);
    require_success(command.status()?, "tauri build")
}

/// Stages the built bundle in `/Applications`, then replaces the installed application.
fn install_app(paths: &Paths) -> Result<()> {
    let temporary = tempfile::tempdir_in("/Applications")
        .context("failed to create temporary application directory")?;
    let temporary_app = temporary.path().join("Wizard.app");

    let status = Command::new("ditto")
        .arg(&paths.bundled_app)
        .arg(&temporary_app)
        .status()
        .context("failed to run ditto")?;
    if !status.success() {
        bail!("failed to copy application into /Applications");
    }

    let result = (|| {
        remove_path(&paths.installed_app)?;
        fs::rename(&temporary_app, &paths.installed_app)?;
        Ok::<_, anyhow::Error>(())
    })();
    result.context("failed to install /Applications/Wizard.app")
}
