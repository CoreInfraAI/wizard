use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context as _, Result, bail};
use semver::Version;
use serde_json::Value;
use tempfile::TempDir;

const APP_NAME: &str = "ff-wizard.app";
const INSTALLED_APP: &str = "/Applications/ff-wizard.app";
const INSTALLED_EXECUTABLE: &str = "/Applications/ff-wizard.app/Contents/MacOS/ff-wizard";

pub(crate) struct Paths {
    pub(crate) workspace: PathBuf,
    pub(crate) wizard: PathBuf,
    pub(crate) tauri_config: PathBuf,
    pub(crate) bundled_app: PathBuf,
    pub(crate) bundle_dir: PathBuf,
    pub(crate) installed_app: PathBuf,
    pub(crate) installed_executable: PathBuf,
}

/// Resolves workspace paths and selects the Cargo output directory for the requested profile.
pub(crate) fn paths(release: bool) -> Result<Paths> {
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
/// Reads the stable base version from `tauri.conf.json` and rejects pre-release versions.
pub(crate) fn stable_version(paths: &Paths) -> Result<Version> {
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

pub(crate) fn temporary_configs() -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix("ff-wizard-tauri.")
        .tempdir_in("/tmp")
        .context("failed to create temporary Tauri config directory")
}

pub(crate) fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(crate) fn files_recursively(root: &Path) -> Result<Vec<PathBuf>> {
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

pub(crate) fn require_success(status: ExitStatus, action: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("{action} exited with {status}")
    }
}

pub(crate) fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} must be set"))
}

pub(crate) fn gh_api_json(endpoint: &str) -> Result<Value> {
    gh_api_json_optional(endpoint)?
        .with_context(|| format!("GitHub resource not found: {endpoint}"))
}

fn gh_api_json_optional(endpoint: &str) -> Result<Option<Value>> {
    let output = Command::new("gh")
        .args(["api", endpoint])
        .output()
        .context("failed to run gh api")?;
    if output.status.success() {
        return serde_json::from_slice(&output.stdout)
            .context("gh api returned invalid JSON")
            .map(Some);
    }

    let response: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    let not_found = response.as_ref().is_some_and(|value| {
        value["status"].as_str() == Some("404") || value["status"].as_u64() == Some(404)
    }) || String::from_utf8_lossy(&output.stderr).contains("HTTP 404");
    if not_found {
        return Ok(None);
    }

    bail!(
        "gh api {endpoint} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

pub(crate) fn gh_api_bytes(endpoint: &str) -> Result<Vec<u8>> {
    let output = Command::new("gh")
        .args([
            "api",
            endpoint,
            "--header",
            "Accept: application/octet-stream",
        ])
        .output()
        .context("failed to download GitHub release asset")?;
    if !output.status.success() {
        bail!(
            "gh api {endpoint} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

pub(crate) fn gh_release_upload(repository: &str, tag: &str, asset: &Path) -> Result<()> {
    let status = Command::new("gh")
        .args(["release", "upload", tag])
        .arg(asset)
        .args(["--clobber", "--repo", repository])
        .status()
        .context("failed to run gh release upload")?;
    require_success(status, "gh release upload")
}
