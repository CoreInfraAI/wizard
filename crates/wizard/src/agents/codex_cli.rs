use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform::macos::command_output;
#[cfg(target_os = "macos")]
use crate::toml;
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CodexCli {
    pub path: PathBuf,
    pub version: String,
    pub proxy_installed: bool,
}

#[cfg(target_os = "macos")]
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
    detect_cli()
}

pub(super) fn set_proxy(installed: bool) -> Result<(), String> {
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
        Err("not supported".to_owned())
    }
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
    let version = match get_cli_version(&path) {
        Ok(value) => value,
        Err(value) => return value,
    };

    let proxy_installed = match config_path().and_then(|path| proxy_installed(&path)) {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(error),
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
fn get_cli_version(path: &PathBuf) -> Result<String, AgentDetection<CodexCli>> {
    let version_output = match command_output(&path, &["--version"]) {
        Ok(output) => output,
        Err(error) => return Err(AgentDetection::Error(error)),
    };
    if !version_output.status.success() {
        return Err(AgentDetection::failed(
            &path,
            &format!(
                "--version exited with {}: {}",
                version_output.status,
                String::from_utf8_lossy(&version_output.stderr).trim()
            ),
        ));
    }
    let bytes = &version_output.stdout;
    let text = match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => return Err(AgentDetection::failed(&path, &error)),
    };
    let version_result = text
        .trim()
        .strip_prefix("codex-cli ")
        .filter(|version1| !version1.is_empty() && !version1.chars().any(char::is_whitespace))
        .map(str::to_owned)
        .ok_or_else(|| format!("unexpected Codex version output: {}", text.trim()));
    let version = match version_result {
        Ok(version) => version,
        Err(error) => return Err(AgentDetection::failed(&path, &error)),
    };
    Ok(version)
}

#[cfg(target_os = "macos")]
fn config_path() -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_owned())?;
    Ok(PathBuf::from(home).join(".codex/config.toml"))
}

#[cfg(target_os = "macos")]
fn proxy_installed(path: &Path) -> Result<bool, String> {
    let doc = toml::read(path)?;
    Ok(PROXY_SETTINGS
        .iter()
        .all(|(keys, value)| toml::get_string(&doc, keys) == Some(*value)))
}
