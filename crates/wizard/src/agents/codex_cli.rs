use super::AgentDetection;

#[cfg(target_os = "macos")]
use crate::platform::macos::command_output;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CodexCli {
    pub path: PathBuf,
    pub version: String,
    pub proxy_installed: bool,
}

pub(super) fn detect() -> AgentDetection<CodexCli> {
    detect_cli()
}

#[cfg(target_os = "macos")]
fn detect_cli() -> AgentDetection<CodexCli> {
    let found = match command_output(Path::new("/usr/bin/which"), &["codex"]) {
        Ok(output) => output,
        Err(error) => return AgentDetection::Error(error),
    };
    if found.status.code() == Some(1) {
        return AgentDetection::NotFound;
    }
    if !found.status.success() {
        return AgentDetection::Error(format!(
            "which codex exited with {}: {}",
            found.status,
            String::from_utf8_lossy(&found.stderr).trim()
        ));
    }
    let path = match core::str::from_utf8(&found.stdout) {
        Ok(value) if !value.trim_end_matches(['\r', '\n']).is_empty() => {
            PathBuf::from(value.trim_end_matches(['\r', '\n']))
        }
        _ => return AgentDetection::Error("which codex returned an invalid path".to_owned()),
    };
    let version_output = match command_output(&path, &["--version"]) {
        Ok(output) => output,
        Err(error) => return AgentDetection::Error(error),
    };
    if !version_output.status.success() {
        return AgentDetection::failed(
            &path,
            &format!(
                "--version exited with {}: {}",
                version_output.status,
                String::from_utf8_lossy(&version_output.stderr).trim()
            ),
        );
    }
    match parse_version(&version_output.stdout) {
        Ok(version) => AgentDetection::Found(CodexCli {
            path,
            version,
            proxy_installed: false,
        }),
        Err(error) => AgentDetection::failed(&path, &error),
    }
}

#[cfg(target_os = "macos")]
fn parse_version(bytes: &[u8]) -> Result<String, String> {
    let text = core::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    text.trim()
        .strip_prefix("codex-cli ")
        .filter(|version| !version.is_empty() && !version.chars().any(char::is_whitespace))
        .map(str::to_owned)
        .ok_or_else(|| format!("unexpected Codex version output: {}", text.trim()))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn detect_cli() -> AgentDetection<CodexCli> {
    AgentDetection::Error("not supported".to_owned())
}
