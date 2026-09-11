use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail};
use semver::Version;
use serde_json::json;

use crate::utils::{
    Paths, files_recursively, remove_path, require_success, stable_version, temporary_configs,
};

const TEST_PRIVATE_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHJzaWduIGVuY3J5cHRlZCBzZWNyZXQga2V5ClJXUlRZMEl5TmlGT0xMc0FVYnN1aXZzWHZlWU9Ra3FzcFp3R1IwNGVDdk8rTTZsRTlTQUFBQkFBQUFBQUFBQUFBQUlBQUFBQUxqTG12cnlGellHNHRkNWo4ejdhRUt5YzdlUmZQZFg1dys2WE1QVHI5YWx6WnA5aTI2SlZrdWtwVDZ0emFCcTRnKy9DWFRZc2UvbGFHelVvaUY2dDNTNzJKanJZYVkrSjN4MTlQMHJXUUQxODg4ZTZWOS91Q0dCV1JDMlBVbHlIRmFuUnRBT3lVNzA9Cg==";
const TEST_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEVFREVCMTI0NjlGQzY5QkUKUldTK2FmeHBKTEhlN3N6SWd1MG9QMU1XdmlydzhROEM3Q1dwdjhUbEFVbFBEV0hTczhKcCtBV3QK";
const TEST_ENDPOINT: &str = "http://127.0.0.1:49173/latest.json";
const DEV_ENDPOINT: &str = "https://coreinfraai.github.io/wizard/latest-dev.json";

/// Runs the application through Tauri with the dev updater channel configured.
pub(crate) fn run(paths: &Paths, release: bool) -> Result<()> {
    let config = json!({
        "bundle": { "createUpdaterArtifacts": false },
        "plugins": {
            "updater": {
                "endpoints": [DEV_ENDPOINT],
                "dangerousInsecureTransportProtocol": false,
            },
        },
    })
    .to_string();

    let mut command = Command::new("cargo");
    command.arg("tauri").arg("dev");
    if release {
        command.arg("--release");
    }
    command
        .args(["--config", &config])
        .current_dir(&paths.wizard);
    require_success(command.status()?, "cargo tauri dev")
}

/// Installs the application when needed and launches it directly or via `LaunchServices`.
pub(crate) fn app(paths: &Paths, reinstall: bool, console: bool, release: bool) -> Result<()> {
    if reinstall || !paths.installed_executable.is_file() {
        let config = json!({
            "bundle": { "createUpdaterArtifacts": false },
            "plugins": {
                "updater": {
                    "endpoints": [DEV_ENDPOINT],
                    "dangerousInsecureTransportProtocol": false,
                },
            },
        })
        .to_string();
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

/// Builds a signed development update and serves its metadata and archive over HTTP.
pub(crate) fn update_server(paths: &Paths, release: bool) -> Result<()> {
    let version = next_update_version(paths)?;
    let temporary = temporary_configs()?;
    let serve_dir = temporary.path().join("server");
    let config = json!({
        "version": version.to_string(),
        "bundle": { "createUpdaterArtifacts": true },
        "plugins": {
            "updater": {
                "endpoints": [TEST_ENDPOINT],
                "pubkey": TEST_PUBLIC_KEY,
                "dangerousInsecureTransportProtocol": true,
            },
        },
    })
    .to_string();

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

/// Builds the app bundle, optionally signing updater artifacts with the test key.
fn build_app(paths: &Paths, config: &str, test_signing: bool, release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.arg("tauri").arg("build");
    if !release {
        command.arg("--debug");
    }
    command
        .args(["--bundles", "app", "--config", config])
        .current_dir(&paths.wizard);
    if test_signing {
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
