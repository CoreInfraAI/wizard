use crate::{
    config_files::{
        self,
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
    platform::env_var_not_empty,
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};
use toml_edit::DocumentMut;

use super::{
    AgentDetection,
    detection::{self, Agent, AgentInfo},
};

static PROXY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Disabled,
    ProxyHub,
    ProxyApi,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Codex {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub proxy_mode: ProxyMode,
}

pub(super) fn detect() -> AgentDetection<Codex> {
    let info = match detection::detect(Agent::Codex) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let proxy_mode = match proxy_mode() {
        Ok(proxy_mode) => proxy_mode,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Codex { info, proxy_mode })
}

fn config_path() -> Result<PathBuf> {
    if let Some(home) = env_var_not_empty("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(".codex/config.toml"))
}

fn proxy_mode() -> Result<ProxyMode> {
    let _guard = PROXY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Codex proxy lock poisoned"))?;
    let doc = config_files::toml::read(&config_path()?)?;
    Ok(mode_from_config(&doc))
}

fn mode_from_config(doc: &DocumentMut) -> ProxyMode {
    match config_files::toml::get_string(doc, &["model_provider"]) {
        Some("coreinfra") => ProxyMode::ProxyHub,
        Some("coreinfra-subscription") => ProxyMode::ProxyApi,
        _ => ProxyMode::Disabled,
    }
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::get_backups(AgentKind::Codex)
}

pub fn restore_backup(id: u32) -> Result<()> {
    let _guard = PROXY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Codex proxy lock poisoned"))?;
    BackupManager::load_backup(AgentKind::Codex, id)
}

pub fn set_proxy(mode: ProxyMode, token: &str) -> Result<()> {
    // Serialize the whole TOML + dotenv operation, not individual file writes.
    let _guard = PROXY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Codex proxy lock poisoned"))?;
    let path = config_path()?;

    let mut config = FileChange::read(path.clone())?;
    let mut dotenv = FileChange::read(path.with_file_name(".env"))?;

    let mut current_mode = ProxyMode::Disabled;
    config_files::toml::update(&mut config.after, |doc| {
        current_mode = mode_from_config(doc);
        remove_inactive_proxy_settings(doc, current_mode, mode)?;
        write_proxy_settings(doc, mode)
    })?;

    remove_inactive_proxy_env(&mut dotenv.after, current_mode, mode)?;
    write_proxy_env(&mut dotenv.after, mode, token)?;
    BackupManager::apply_changes(AgentKind::Codex, &[config, dotenv])
}

const API_FILTERS: &[&str] = &[
    "COREINFRA_API_KEY",
    "COREINFRA_PROXY_URL",
    "HTTPS_PROXY",
    "NO_PROXY",
];

fn remove_inactive_proxy_settings(
    doc: &mut DocumentMut,
    current_mode: ProxyMode,
    _mode: ProxyMode,
) -> Result<()> {
    if current_mode != ProxyMode::Disabled {
        config_files::toml::remove(doc, &["model_provider"])?;
    }

    config_files::toml::remove(doc, &["model_providers", "coreinfra"])?;
    config_files::toml::remove(doc, &["model_providers", "coreinfra-subscription"])?;

    if current_mode == ProxyMode::ProxyHub {
        config_files::toml::remove(doc, &["features", "api_key_model_discovery"])?;
    }
    if current_mode == ProxyMode::ProxyApi {
        config_files::toml::remove(doc, &["features", "shell_snapshot"])?;
        for key in API_FILTERS {
            config_files::toml::remove(doc, &["shell_environment_policy", "filters", key])?;
        }
    }
    Ok(())
}

fn write_proxy_settings(doc: &mut DocumentMut, mode: ProxyMode) -> Result<()> {
    let (provider, name, base_url) = match mode {
        ProxyMode::Disabled => return Ok(()),
        ProxyMode::ProxyHub => (
            "coreinfra",
            "CoreInfra AI Hub",
            "https://hub.coreinfra.ai/codex/api/v1",
        ),
        ProxyMode::ProxyApi => (
            "coreinfra-subscription",
            "CoreInfra AI Hub | ChatGPT",
            "https://hub.coreinfra.ai/openai/subscription/api/v1",
        ),
    };
    config_files::toml::set_value(doc, &["model_provider"], provider)?;
    for (key, value) in [
        ("name", name),
        ("base_url", base_url),
        ("wire_api", "responses"),
    ] {
        config_files::toml::set_value(doc, &["model_providers", provider, key], value)?;
    }

    match mode {
        ProxyMode::ProxyHub => {
            config_files::toml::set_value(doc, &["features", "api_key_model_discovery"], true)?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "env_key"],
                "COREINFRA_API_KEY",
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "model_catalog_url"],
                "https://hub.coreinfra.ai/codex/api/v1/models",
            )?;
        }
        ProxyMode::ProxyApi => {
            config_files::toml::set_value(doc, &["features", "shell_snapshot"], false)?;
            for key in API_FILTERS {
                config_files::toml::set_value(
                    doc,
                    &["shell_environment_policy", "filters", key],
                    "exclude",
                )?;
            }
            let mut headers = toml_edit::InlineTable::new();
            headers.insert("X-CoreInfra-API-Key", "COREINFRA_API_KEY".into());
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "env_http_headers"],
                headers,
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "requires_openai_auth"],
                true,
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "supports_websockets"],
                true,
            )?;
        }
        ProxyMode::Disabled => {}
    }
    config_files::toml::implicit_table(doc, &["model_providers"])?;
    Ok(())
}

fn remove_inactive_proxy_env(
    after: &mut FileSnapshot,
    current_mode: ProxyMode,
    mode: ProxyMode,
) -> Result<()> {
    if mode == ProxyMode::Disabled {
        config_files::env::set_many(after, &[("COREINFRA_API_KEY", None)])?;
    }
    if current_mode == ProxyMode::ProxyApi && mode != ProxyMode::ProxyApi {
        config_files::env::set_many(
            after,
            &[
                ("COREINFRA_PROXY_URL", None),
                ("HTTPS_PROXY", None),
                ("NO_PROXY", None),
            ],
        )?;
    }
    Ok(())
}

fn write_proxy_env(after: &mut FileSnapshot, mode: ProxyMode, token: &str) -> Result<()> {
    match mode {
        ProxyMode::Disabled => Ok(()),
        ProxyMode::ProxyHub => {
            config_files::env::set_many(after, &[("COREINFRA_API_KEY", Some(token))])
        }
        ProxyMode::ProxyApi => config_files::env::set_many(
            after,
            &[
                ("COREINFRA_API_KEY", Some(token)),
                (
                    "COREINFRA_PROXY_URL",
                    Some("https://coreinfra:${COREINFRA_API_KEY}@proxy.hub.coreinfra.ai:443"),
                ),
                ("HTTPS_PROXY", Some("${COREINFRA_PROXY_URL}")),
                ("NO_PROXY", Some("localhost,127.0.0.1,::1,hub.coreinfra.ai")),
            ],
        ),
    }
}
