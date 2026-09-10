use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
};

use anyhow::{Context as _, Result, bail};
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
