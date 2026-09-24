use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform::macos::command_output;
use crate::toml;
use anyhow::{Context as _, Result};
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CodexCli {
    pub path: PathBuf,
    pub version: String,
    pub proxy_installed: bool,
}

#[cfg_attr(any(target_os = "linux", target_os = "windows"), expect(dead_code))]
const PROXY_SETTINGS: &[(&[&str], &str)] = &[
    (&["model_provider"], "coreinfra"),
    (
        &["model_providers", "coreinfra", "name"],
        "CoreInfra AI Hub",
    ),
    (
        &["model_providers", "coreinfra", "base_url"],
        "https://hub.coreinfra.ai/codex/api/v1",
    ),
    (&["model_providers", "coreinfra", "wire_api"], "responses"),
    (
        &["model_providers", "coreinfra", "env_key"],
        "COREINFRA_API_KEY",
    ),
    (
        &[
            "model_providers",
            "coreinfra",
            "http_headers",
            "X-CoreInfra-CrossProtocol",
        ],
        "1",
    ),
];

pub(super) fn detect() -> AgentDetection<CodexCli> {
    let result = detect_cli();
    match &result {
        AgentDetection::Found(cli) => log::debug!(
            "found Codex CLI: path={}, version={}, proxy_installed={}",
            cli.path.display(),
            cli.version,
            cli.proxy_installed
        ),
        AgentDetection::NotFound => log::debug!("Codex CLI not found in PATH"),
        AgentDetection::Error(error) => log::error!("Codex CLI detection failed: {error}"),
    }
    result
}

pub(super) fn set_proxy(installed: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let path = &config_path()?;
        toml::update(path, |doc| {
            if installed {
                for (keys, value) in PROXY_SETTINGS {
                    toml::set_string(doc, keys, value)?;
                }
                toml::implicit_table(doc, &["model_providers"])?;
                toml::inline_table(doc, &["model_providers", "coreinfra", "http_headers"])?;
            } else {
                if toml::get_string(doc, &["model_provider"]) == Some("coreinfra") {
                    toml::remove(doc, &["model_provider"])?;
                }
                toml::remove(doc, &["model_providers", "coreinfra"])?;
            }
            Ok(())
        })
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let _ = installed;
        anyhow::bail!("not supported")
    }
}

#[cfg(target_os = "macos")]
fn detect_cli() -> AgentDetection<CodexCli> {
    let found = match command_output(Path::new("/usr/bin/which"), &["codex"]) {
        Ok(output) => output,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
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
    let version = match get_cli_version(&path) {
        Ok(value) => value,
        Err(error) => return AgentDetection::failed(&path, &error),
    };

    let proxy_installed = match config_path().and_then(|path| proxy_installed(&path)) {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(CodexCli {
        path,
        version,
        proxy_installed,
    })
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn detect_cli() -> AgentDetection<CodexCli> {
    AgentDetection::Error("not supported".to_owned())
}

#[cfg(target_os = "macos")]
fn get_cli_version(path: &Path) -> Result<String> {
    let output = command_output(path, &["--version"])?;
    anyhow::ensure!(
        output.status.success(),
        "--version exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text =
        core::str::from_utf8(&output.stdout).context("invalid UTF-8 in Codex version output")?;
    text.trim()
        .strip_prefix("codex-cli ")
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("unexpected Codex version output: {}", text.trim()))
}

#[cfg_attr(any(target_os = "linux", target_os = "windows"), expect(dead_code))]
fn config_path() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".codex/config.toml"))
}

#[cfg_attr(any(target_os = "linux", target_os = "windows"), expect(dead_code))]
fn proxy_installed(path: &Path) -> Result<bool> {
    let doc = toml::read(path)?;
    Ok(PROXY_SETTINGS
        .iter()
        .all(|(keys, value)| toml::get_string(&doc, keys) == Some(*value)))
}
