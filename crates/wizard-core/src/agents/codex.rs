use super::AgentDetection;
use crate::config_files;
use crate::platform::{command_output, find_executable};
use anyhow::{Context as _, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Codex {
    pub path: PathBuf,
    pub version: String,
    pub proxy_installed: bool,
}

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
        &["model_providers", "coreinfra", "model_catalog_url"],
        "https://hub.coreinfra.ai/codex/api/v1/models",
    ),
];

pub(super) fn detect() -> AgentDetection<Codex> {
    let path = match find_executable("codex") {
        Ok(Some(path)) => path,
        Ok(None) => return AgentDetection::NotFound,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    let version = match get_codex_version(&path) {
        Ok(value) => value,
        Err(error) => return AgentDetection::failed(&path, &error),
    };

    let proxy_installed = match proxy_installed() {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Codex {
        path,
        version,
        proxy_installed,
    })
}

fn get_codex_version(path: &Path) -> Result<String> {
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

fn config_path() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(".codex/config.toml"))
}

fn proxy_installed() -> Result<bool> {
    let doc = config_files::toml::read(&config_path()?)?;
    Ok(PROXY_SETTINGS
        .iter()
        .all(|(keys, value)| config_files::toml::get_string(&doc, keys) == Some(*value))
        && config_files::toml::get_value(&doc, &["features", "api_key_model_discovery"])
            .and_then(toml_edit::Value::as_bool)
            == Some(true))
}

pub fn set_proxy(install: bool, token: &str) -> Result<()> {
    let path = &config_path()?;
    config_files::toml::update(path, |doc| {
        if install {
            for (keys, value) in PROXY_SETTINGS {
                config_files::toml::set_value(doc, keys, *value)?;
            }
            config_files::toml::implicit_table(doc, &["model_providers"])?;
            config_files::toml::set_value(doc, &["features", "api_key_model_discovery"], true)?;
            config_files::toml::remove(
                doc,
                &[
                    "model_providers",
                    "coreinfra",
                    "http_headers",
                    "X-CoreInfra-CrossProtocol",
                ],
            )?;
        } else {
            if config_files::toml::get_string(doc, &["model_provider"]) == Some("coreinfra") {
                config_files::toml::remove(doc, &["model_provider"])?;
            }
            config_files::toml::remove(doc, &["model_providers", "coreinfra"])?;
            config_files::toml::remove(doc, &["features", "api_key_model_discovery"])?;
        }
        Ok(())
    })?;
    config_files::env::set(
        &path.with_file_name(".env"),
        "COREINFRA_API_KEY",
        if install && !token.is_empty() {
            Some(token)
        } else {
            None
        },
    )
    .context("Codex config updated, but updating Codex .env failed; please retry")
}
