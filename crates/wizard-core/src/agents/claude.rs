use super::{
    AgentDetection,
    detection::{self, Agent, AgentInfo},
};
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
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

const HUB_URL: &str = "https://hub.coreinfra.ai/claude/api";
const API_URL: &str = "https://hub.coreinfra.ai/anthropic/subscription/api";
const NO_PROXY_SCRIPT: &str = "#!/bin/sh\n\nunset HTTPS_PROXY NO_PROXY\n";
static PROXY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Disabled,
    ProxyHub,
    ProxyApi,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Claude {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub proxy_mode: ProxyMode,
}

pub(super) fn detect() -> AgentDetection<Claude> {
    let info = match detection::detect(Agent::Claude) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let proxy_mode = match proxy_mode() {
        Ok(mode) => mode,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Claude { info, proxy_mode })
}

fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = env_var_not_empty("CLAUDE_CONFIG_DIR") {
        return std::path::absolute(dir).context("failed to resolve Claude config directory");
    }
    Ok(dirs::home_dir()
        .context("failed to resolve home directory")?
        .join(".claude"))
}

fn global_config_path() -> Result<PathBuf> {
    if let Some(dir) = env_var_not_empty("CLAUDE_CONFIG_DIR") {
        return std::path::absolute(PathBuf::from(dir).join(".claude.json"))
            .context("failed to resolve Claude global config path");
    }
    Ok(dirs::home_dir()
        .context("failed to resolve home directory")?
        .join(".claude.json"))
}

fn proxy_mode() -> Result<ProxyMode> {
    let _guard = PROXY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Claude proxy lock poisoned"))?;
    let settings = config_files::json::read(&config_dir()?.join("settings.json"))?;
    Ok(mode_from_config(&settings))
}

fn mode_from_config(settings: &Value) -> ProxyMode {
    match settings["env"]["ANTHROPIC_BASE_URL"].as_str() {
        Some(HUB_URL) => ProxyMode::ProxyHub,
        Some(API_URL) => ProxyMode::ProxyApi,
        _ => ProxyMode::Disabled,
    }
}

pub fn set_proxy(mode: ProxyMode, token: &str) -> Result<()> {
    let _guard = PROXY_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Claude proxy lock poisoned"))?;
    let dir = config_dir()?;
    let script_path = dir.join("no-proxy.sh");
    let mut settings = FileChange::read(dir.join("settings.json"))?;
    let mut global = FileChange::read(global_config_path()?)?;
    let mut script = FileChange::read(script_path.clone())?;

    let current_mode = mode_from_config(&config_files::json::parse(&settings.after)?);
    if !(mode == ProxyMode::Disabled && current_mode == ProxyMode::Disabled) {
        config_files::json::update(&mut settings.after, |settings| {
            let env = settings
                .as_object_mut()
                .context("Claude settings must be an object")?
                .entry("env")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("Claude env must be an object")?;
            remove_inactive_proxy_settings(env, current_mode);
            write_proxy_settings(env, mode, token, &script_path)
        })?;
    }
    if mode == ProxyMode::ProxyHub {
        // The saved token is ASCII; Claude approves its last 20 characters.
        let suffix = token.get(token.len().saturating_sub(20)..).unwrap_or(token);
        approve_key(&mut global.after, suffix)?;
    }
    if mode == ProxyMode::ProxyApi {
        script.after = FileSnapshot::new(NO_PROXY_SCRIPT.to_owned(), 0o700);
    }
    if mode != ProxyMode::ProxyApi && current_mode == ProxyMode::ProxyApi {
        script.after = FileSnapshot::Missing;
    }

    // Keep preparation files before activation, and script deletion after it.
    let changes = if mode == ProxyMode::ProxyApi {
        [global, script, settings]
    } else {
        [global, settings, script]
    };
    BackupManager::apply_changes(AgentKind::Claude, &changes)
}

fn remove_inactive_proxy_settings(env: &mut Map<String, Value>, current_mode: ProxyMode) {
    // Only clean up fields owned by the previously selected CoreInfra mode.
    let previous_keys: &[&str] = match current_mode {
        ProxyMode::Disabled => &[],
        ProxyMode::ProxyHub => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        ],
        ProxyMode::ProxyApi => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "HTTPS_PROXY",
            "NO_PROXY",
            "CLAUDE_ENV_FILE",
        ],
    };
    for key in previous_keys {
        env.remove(*key);
    }
}

fn write_proxy_settings(
    env: &mut Map<String, Value>,
    mode: ProxyMode,
    token: &str,
    script_path: &Path,
) -> Result<()> {
    match mode {
        ProxyMode::Disabled => {}
        ProxyMode::ProxyHub => {
            env.insert("ANTHROPIC_BASE_URL".into(), json!(HUB_URL));
            env.insert("ANTHROPIC_API_KEY".into(), json!(token));
            env.insert(
                "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
                json!("1"),
            );
        }
        ProxyMode::ProxyApi => {
            // Encode the password component; the header keeps the literal token.
            let password: String = token
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                        char::from(byte).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect();
            env.insert("ANTHROPIC_BASE_URL".into(), json!(API_URL));
            env.insert(
                "ANTHROPIC_CUSTOM_HEADERS".into(),
                json!(format!("X-CoreInfra-API-Key: {token}")),
            );
            env.insert(
                "HTTPS_PROXY".into(),
                json!(format!(
                    "https://coreinfra:{password}@proxy.hub.coreinfra.ai:443"
                )),
            );
            env.insert(
                "NO_PROXY".into(),
                json!("localhost,127.0.0.1,::1,hub.coreinfra.ai"),
            );
            env.insert(
                "CLAUDE_ENV_FILE".into(),
                json!(
                    script_path
                        .to_str()
                        .context("Claude script path is not valid UTF-8")?
                ),
            );
        }
    }
    Ok(())
}

fn approve_key(after: &mut FileSnapshot, suffix: &str) -> Result<()> {
    config_files::json::update(after, |config| {
        config["hasCompletedOnboarding"] = json!(true);
        let approved = config
            .as_object_mut()
            .context("Claude config must be an object")?
            .entry("customApiKeyResponses")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Claude customApiKeyResponses must be an object")?
            .entry("approved")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Claude approved keys must be an array")?;
        if !approved.iter().any(|value| value.as_str() == Some(suffix)) {
            approved.push(json!(suffix));
        }
        // Preserve old approvals and onboarding when switching away from Hub.
        Ok(())
    })
}
