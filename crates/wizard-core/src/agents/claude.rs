use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files::{
        self,
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
    platform::env_var_not_empty,
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use wizard_settings::Settings;

const HUB_URL: &str = "https://hub.coreinfra.ai/claude/api";
const API_URL: &str = "https://hub.coreinfra.ai/anthropic/subscription/api";
const NO_PROXY_SCRIPT: &str = "#!/bin/sh\n\nunset HTTPS_PROXY NO_PROXY\n";
const TOKEN_HEADER: &str = "X-CoreInfra-API-Key: ";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Claude {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<Claude> {
    let info = match detection::detect(Agent::Claude, settings.last_paths.claude.as_deref()) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let (routing_mode, token) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Claude {
        info,
        routing_mode,
        token,
    })
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

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

fn read_routing() -> Result<(RoutingMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::Claude.lock()?;
    let settings = config_files::json::read(&config_dir()?.join("settings.json"))?;
    let mode = mode_from_config(&settings);
    Ok((mode, token_from_config(&settings, mode)))
}

/// Reads only keys owned by the active mode: a disabled API key may belong to the user.
fn token_from_config(settings: &Value, mode: RoutingMode) -> Option<AgentToken> {
    let env = &settings["env"];
    let token = match mode {
        RoutingMode::Disabled => return None,
        RoutingMode::Hub => env["ANTHROPIC_API_KEY"].as_str()?,
        RoutingMode::Sub => env["ANTHROPIC_CUSTOM_HEADERS"]
            .as_str()?
            .lines()
            .find_map(|header| header.strip_prefix(TOKEN_HEADER))?,
    };
    AgentToken::new(token.to_owned())
}

fn mode_from_config(settings: &Value) -> RoutingMode {
    match settings["env"]["ANTHROPIC_BASE_URL"].as_str() {
        Some(HUB_URL) => RoutingMode::Hub,
        Some(API_URL) => RoutingMode::Sub,
        _ => RoutingMode::Disabled,
    }
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::get_backups(AgentKind::Claude)
}

pub fn restore_backup(id: u32) -> Result<()> {
    let _guard = PreparedAgent::Claude.lock()?;
    BackupManager::load_backup(AgentKind::Claude, id)
}

/// `token` is ignored when disabled.
pub fn prepare_routing(mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::Claude.lock()?;

    let dir = config_dir()?;
    let script_path = dir.join("no-proxy.sh");

    let mut settings = FileChange::read(dir.join("settings.json"))?;
    let mut global = FileChange::read(global_config_path()?)?;
    let mut script = FileChange::read(script_path.clone())?;

    let current_mode = mode_from_config(&config_files::json::parse(&settings.after)?);
    if !(mode == RoutingMode::Disabled && current_mode == RoutingMode::Disabled) {
        config_files::json::update(&mut settings.after, |settings| {
            let env = settings
                .as_object_mut()
                .context("Claude settings must be an object")?
                .entry("env")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("Claude env must be an object")?;
            remove_inactive_routing_settings(env, current_mode);
            write_routing_settings(env, mode, token, &script_path)
        })?;
    }
    if mode == RoutingMode::Hub {
        // The saved token is ASCII; Claude approves its last 20 characters.
        let suffix = token.get(token.len().saturating_sub(20)..).unwrap_or(token);
        approve_key(&mut global.after, suffix)?;
    }
    if mode == RoutingMode::Sub {
        script.after = FileSnapshot::new(NO_PROXY_SCRIPT.to_owned(), 0o700);
    }
    if mode != RoutingMode::Sub && current_mode == RoutingMode::Sub {
        script.after = FileSnapshot::Missing;
    }

    // Keep preparation files before activation, and script deletion after it.
    let changes = if mode == RoutingMode::Sub {
        [global, script, settings]
    } else {
        [global, settings, script]
    };
    Ok(prepare_plan(changes.into()))
}

fn prepare_plan(changes: Vec<FileChange>) -> PreparedPlan {
    let steps = changes
        .into_iter()
        .map(|change| PreparedStep::ChangeFile { change })
        .collect();
    PreparedPlan::new(PreparedAgent::Claude, steps)
}

fn remove_inactive_routing_settings(env: &mut Map<String, Value>, current_mode: RoutingMode) {
    // Only clean up fields owned by the previously selected CoreInfra mode.
    let previous_keys: &[&str] = match current_mode {
        RoutingMode::Disabled => &[],
        RoutingMode::Hub => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
            "ANTHROPIC_DEFAULT_MODEL",
        ],
        RoutingMode::Sub => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "HTTPS_PROXY",
            "NO_PROXY",
            "CLAUDE_ENV_FILE",
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
            "ANTHROPIC_DEFAULT_MODEL",
        ],
    };
    for key in previous_keys {
        env.shift_remove(*key);
    }
}

fn write_routing_settings(
    env: &mut Map<String, Value>,
    mode: RoutingMode,
    token: &str,
    script_path: &Path,
) -> Result<()> {
    if mode != RoutingMode::Disabled {
        // Off by default behind a custom base URL; without it Anthropic buffers whole tool inputs.
        env.insert(
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING".into(),
            json!("1"),
        );
        // Lowest-priority default: `/model`, `--model` and `ANTHROPIC_MODEL` still win.
        env.insert("ANTHROPIC_DEFAULT_MODEL".into(), json!("sonnet"));
    }

    match mode {
        RoutingMode::Disabled => {}
        RoutingMode::Hub => {
            env.insert("ANTHROPIC_BASE_URL".into(), json!(HUB_URL));
            env.insert("ANTHROPIC_API_KEY".into(), json!(token));
            env.insert(
                "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
                json!("1"),
            );
        }
        RoutingMode::Sub => {
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
                json!(format!("{TOKEN_HEADER}{token}")),
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
