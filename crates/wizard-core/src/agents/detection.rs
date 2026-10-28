//! Discovery of CLI and desktop agents.

use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform::env_var_not_empty;
use crate::platform::{append_command_path, command_output, find_executable};
use anyhow::{Context as _, Result};
use core::time::Duration;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Clone, Copy, Debug)]
pub(super) enum Agent {
    Codex,
    Claude,
    OpenCode,
    Pi,
}

#[cfg(target_os = "macos")]
pub(super) const CLAUDE_APP_PATH: &str = "/Applications/Claude.app";

#[derive(Clone, Copy, Debug)]
pub(super) enum DesktopAgent {
    // The existing code identity still refers to the ChatGPT application.
    Codex,
    Claude,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[cfg_attr(target_os = "windows", derive(serde::Deserialize))]
pub(crate) struct AgentInfo {
    pub path: PathBuf,
    // CLI detection always supplies a version; desktop bundles may lack one.
    pub version: Option<String>,
}

/// Must run inside a Tokio blocking worker.
pub(super) fn detect(agent: Agent, last_found: Option<&Path>) -> AgentDetection<AgentInfo> {
    let command = match agent {
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
    }

    let paths = paths.into_iter().map(PathBuf::from).collect();

    let path = match find_executable(command, paths, last_found) {
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
    if let Some(node) = find_executable("node", vec![], None)? {
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
        // OpenCode 2 prints `opencode v2.1.1`; earlier versions print only the version.
        Agent::OpenCode => {
            let text = text.strip_prefix("opencode v").unwrap_or(text);
            (text.split_whitespace().count() == 1).then_some(text)
        }
        Agent::Pi => (text.split_whitespace().count() == 1).then_some(text),
    };
    version
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("unexpected {agent:?} version output: {text}"))
}

/// Must run inside a Tokio blocking worker. Never launches the desktop application.
pub(super) fn detect_desktop(
    agent: DesktopAgent,
    last_found: Option<&Path>,
) -> AgentDetection<AgentInfo> {
    #[cfg(target_os = "macos")]
    {
        let app_path = Path::new(match agent {
            DesktopAgent::Codex => "/Applications/ChatGPT.app",
            DesktopAgent::Claude => CLAUDE_APP_PATH,
        });
        let app_name = app_path
            .file_name()
            .expect("desktop app path has a filename");
        let mut candidates: Vec<PathBuf> = last_found.into_iter().map(Path::to_owned).collect();
        candidates.push(app_path.to_owned());
        if let Some(home) = env_var_not_empty("HOME") {
            candidates.push(PathBuf::from(home).join("Applications").join(app_name));
        }
        for path in candidates {
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return AgentDetection::failed(&path, &error),
            }
            return match crate::platform::macos::read_app_version(&path) {
                Ok(version) => AgentDetection::Found(AgentInfo { path, version }),
                Err(error) => AgentDetection::failed(&path, &error),
            };
        }
        AgentDetection::NotFound
    }
    #[cfg(target_os = "linux")]
    {
        let command = match agent {
            DesktopAgent::Claude => "claude-desktop",
            DesktopAgent::Codex => "chatgpt",
        };
        // Never start the desktop app with --version: it can open a window instead.
        let path = match find_executable(command, vec![], last_found) {
            Ok(Some(path)) => path,
            Ok(None) => return AgentDetection::NotFound,
            Err(error) => return AgentDetection::Error(format!("{error:#}")),
        };
        // TODO: get ChatGPT version
        let version = if matches!(agent, DesktopAgent::Claude) {
            let mut command = tokio::process::Command::new("dpkg-query");
            command
                .args(["-W", "-f=${Status} ${Version}", "claude-desktop"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let output = match command_output(command, Duration::from_secs(5)) {
                Ok(output) => output,
                Err(error) => return AgentDetection::failed(&path, &error),
            };
            if output.status.success() {
                String::from_utf8(output.stdout).ok().and_then(|text| {
                    text.strip_prefix("install ok installed ")
                        .map(|version| version.trim().to_owned())
                })
            } else {
                None
            }
        } else {
            None
        };

        AgentDetection::Found(AgentInfo { path, version })
    }
    #[cfg(target_os = "windows")]
    {
        let (packages, executable) = match agent {
            DesktopAgent::Claude => ("Claude,AnthropicPBC.Claude", "Claude.exe"),
            DesktopAgent::Codex => ("OpenAI.Codex", "ChatGPT.exe"),
        };
        let info = match detect_desktop_windows(packages, executable, last_found) {
            Ok(Some(info)) => info,
            Ok(None) => return AgentDetection::NotFound,
            Err(error) => return AgentDetection::Error(format!("{error:#}")),
        };
        AgentDetection::Found(info)
    }
}

#[cfg(target_os = "windows")]
fn detect_desktop_windows(
    packages: &str,
    executable: &str,
    last_found: Option<&Path>,
) -> Result<Option<AgentInfo>> {
    // Read installation/version metadata only. Pass the saved path through the
    // environment, not PowerShell source, and never launch the application.
    let script = r"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
function Write-Agent($path) {
    $version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductVersion
    if ([string]::IsNullOrWhiteSpace($version)) { $version = $null }
    @{ path = [System.IO.Path]::GetFullPath($path); version = $version } | ConvertTo-Json -Compress
}
if ($env:WIZARD_DESKTOP_PATH -and (Test-Path -LiteralPath $env:WIZARD_DESKTOP_PATH -PathType Leaf)) {
    Write-Agent $env:WIZARD_DESKTOP_PATH
    exit 0
}
$packages = foreach ($name in $env:WIZARD_DESKTOP_PACKAGES.Split(',')) {
    Get-AppxPackage -Name $name
}
foreach ($package in ($packages | Sort-Object Version -Descending)) {
    $manifest = $package | Get-AppxPackageManifest
    foreach ($application in $manifest.Package.Applications.Application) {
        $relative = [string]$application.Executable
        if ([System.IO.Path]::GetFileName($relative) -ine $env:WIZARD_DESKTOP_EXECUTABLE) { continue }
        $path = Join-Path $package.InstallLocation $relative
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Write-Agent $path
            exit 0
        }
    }
}
'null'
";
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("WIZARD_DESKTOP_PACKAGES", packages)
        .env("WIZARD_DESKTOP_EXECUTABLE", executable)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = last_found {
        command.env("WIZARD_DESKTOP_PATH", path);
    } else {
        command.env_remove("WIZARD_DESKTOP_PATH");
    }
    let output = command_output(command, Duration::from_secs(15))?;
    anyhow::ensure!(output.status.success(), "desktop installation query failed");
    serde_json::from_slice(&output.stdout).context("invalid desktop installation query response")
}
