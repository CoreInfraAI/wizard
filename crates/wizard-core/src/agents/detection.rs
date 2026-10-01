//! Discovery of CLI and desktop agents.

use core::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context as _, Result};
use serde::Serialize;

use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform::env_var_not_empty;
use crate::platform::{append_command_path, command_output, find_executable};

#[derive(Clone, Copy, Debug)]
pub(super) enum Agent {
    Codex,
    ChatGpt,
    Claude,
    ClaudeDesktop,
    OpenCode,
    Pi,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct AgentInfo {
    pub path: PathBuf,
    // CLI detection always supplies a version; desktop bundles may lack one.
    pub version: Option<String>,
}

/// Must run inside a Tokio blocking worker.
pub(super) fn detect(agent: Agent) -> AgentDetection<AgentInfo> {
    let command = match agent {
        Agent::ChatGpt => return detect_desktop("ChatGPT.app"),
        Agent::ClaudeDesktop => return detect_desktop("Claude.app"),
        Agent::Codex => "codex",
        Agent::Claude => "claude",
        Agent::OpenCode => "opencode",
        Agent::Pi => "pi",
    };

    let mut paths = Vec::new();
    match agent {
        Agent::Codex => {
            #[cfg(target_os = "windows")]
            paths.push("%LOCALAPPDATA%/Programs/OpenAI/Codex/bin");
        }
        Agent::Claude => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            paths.push("~/.claude/local");
        }
        Agent::OpenCode => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            paths.push("~/.opencode/bin");
        }
        Agent::Pi => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            paths.push("~/.pi/agent/bin");
        }
        Agent::ChatGpt | Agent::ClaudeDesktop => {}
    }

    let paths = paths.into_iter().map(PathBuf::from).collect();

    let path = match find_executable(command, paths) {
        Ok(Some(path)) => path,
        Ok(None) => return AgentDetection::NotFound,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    let version = match cli_version(agent, &path) {
        Ok(version) => version,
        Err(error) => return AgentDetection::failed(&path, &error),
    };
    AgentDetection::Found(AgentInfo {
        path,
        version: Some(version),
    })
}

fn cli_version(agent: Agent, path: &Path) -> Result<String> {
    let mut command = tokio::process::Command::new(path);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(node) = find_executable("node", vec![])? {
        let directory = node
            .parent()
            .context("Node executable has no parent directory")?;
        append_command_path(&mut command, &[directory])?;
    }
    let output = command_output(command, Duration::from_secs(5))?;
    anyhow::ensure!(
        output.status.success(),
        "--version exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = core::str::from_utf8(&output.stdout)
        .with_context(|| format!("invalid UTF-8 in {agent:?} version output"))?
        .trim();
    let version = match agent {
        Agent::Codex => text.strip_prefix("codex-cli "),
        Agent::Claude => text.strip_suffix(" (Claude Code)"),
        Agent::OpenCode | Agent::Pi => (text.split_whitespace().count() == 1).then_some(text),
        Agent::ChatGpt | Agent::ClaudeDesktop => {
            anyhow::bail!("{agent:?} is not a CLI agent");
        }
    };
    version
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("unexpected {agent:?} version output: {text}"))
}

fn detect_desktop(app_name: &str) -> AgentDetection<AgentInfo> {
    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![PathBuf::from("/Applications").join(app_name)];
        if let Some(home) = env_var_not_empty("HOME") {
            candidates.push(PathBuf::from(home).join("Applications").join(app_name));
        }
        for path in candidates {
            match path.try_exists() {
                Ok(false) => continue,
                Err(error) => return AgentDetection::failed(&path, &error),
                Ok(true) => {}
            }
            return match crate::platform::macos::read_app_version(&path) {
                Ok(version) => AgentDetection::Found(AgentInfo { path, version }),
                Err(error) => AgentDetection::failed(&path, &error),
            };
        }
        AgentDetection::NotFound
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_name;
        AgentDetection::Error("not supported".to_owned())
    }
}
