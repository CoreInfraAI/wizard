use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;

use super::AgentDetection;
use crate::platform::{command_output, find_executable};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Pi {
    pub path: PathBuf,
    pub version: String,
}

pub(super) fn detect() -> AgentDetection<Pi> {
    let path = match find_executable("pi") {
        Ok(Some(path)) => path,
        Ok(None) => return AgentDetection::NotFound,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    match get_pi_version(&path) {
        Ok(version) => AgentDetection::Found(Pi { path, version }),
        Err(error) => AgentDetection::failed(&path, &error),
    }
}

fn get_pi_version(path: &Path) -> Result<String> {
    let output = command_output(path, &["--version"])?;
    anyhow::ensure!(
        output.status.success(),
        "--version exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let version = core::str::from_utf8(&output.stdout)
        .context("invalid UTF-8 in pi version output")?
        .trim();
    anyhow::ensure!(
        !version.is_empty() && version.split_whitespace().count() == 1,
        "unexpected pi version output: {version}"
    );
    Ok(version.to_owned())
}
