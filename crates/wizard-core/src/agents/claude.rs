use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Serialize;

use super::AgentDetection;
use crate::platform::{command_output, find_executable};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Claude {
    pub path: PathBuf,
    pub version: String,
}

pub(super) fn detect() -> AgentDetection<Claude> {
    let path = match find_executable("claude") {
        Ok(Some(path)) => path,
        Ok(None) => return AgentDetection::NotFound,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    match get_claude_version(&path) {
        Ok(version) => AgentDetection::Found(Claude { path, version }),
        Err(error) => AgentDetection::failed(&path, &error),
    }
}

fn get_claude_version(path: &Path) -> Result<String> {
    let output = command_output(path, &["--version"])?;
    anyhow::ensure!(
        output.status.success(),
        "--version exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text =
        core::str::from_utf8(&output.stdout).context("invalid UTF-8 in Claude version output")?;
    text.trim()
        .strip_suffix(" (Claude Code)")
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("unexpected Claude version output: {}", text.trim()))
}
