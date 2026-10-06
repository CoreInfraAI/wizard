use super::{
    detection::{self, Agent, AgentInfo},
    AgentDetection,
};
use crate::{config_files, platform::env_var_not_empty};
use anyhow::{ensure, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::Write as _,
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
    let settings_path = dir.join("settings.json");
    let script_path = dir.join("no-proxy.sh");
    let settings_was = config_files::json::read(&settings_path)?;
    let current_mode = mode_from_config(&settings_was);
    if mode == ProxyMode::Disabled && current_mode == ProxyMode::Disabled {
        return Ok(());
    }
    let mut settings = settings_was.clone();
    let env = settings
        .as_object_mut()
        .context("Claude settings must be an object")?
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Claude env must be an object")?;

    remove_inactive_proxy_settings(env, current_mode);
    write_proxy_settings(env, mode, token, &script_path)?;

    // Prepare the new mode before changing the active settings.
    match mode {
        ProxyMode::Disabled => {}
        ProxyMode::ProxyHub => {
            // token contains only ACSII
            let suffix = token.get(token.len().saturating_sub(20)..).unwrap_or(token);
            approve_key(&global_config_path()?, suffix)?;
        }
        ProxyMode::ProxyApi => write_no_proxy_script(&script_path)?,
    }
    config_files::json::write(&settings_path, &settings_was, &settings)
        .context("Claude preparation completed, but saving proxy settings failed; close Claude Code and retry")?;
    if current_mode == ProxyMode::ProxyApi && mode != ProxyMode::ProxyApi {
        remove_no_proxy_script(&script_path)
            .context("Claude proxy settings saved, but script cleanup failed")?;
    }
    Ok(())
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

fn approve_key(path: &Path, suffix: &str) -> Result<()> {
    let original = config_files::json::read(path)?;
    let mut config = original.clone();
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
    config_files::json::write(path, &original, &config)
}

fn write_no_proxy_script(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "Claude no-proxy.sh must be a regular file"
            );
            ensure!(
                fs::read(path).context("failed to read Claude no-proxy.sh")?
                    == NO_PROXY_SCRIPT.as_bytes(),
                "Claude no-proxy.sh contains custom content; refusing to overwrite it"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .context("failed to set Claude script permissions")?;
            }
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect Claude no-proxy.sh"),
    }
    let parent = path
        .parent()
        .context("Claude script has no parent directory")?;
    fs::create_dir_all(parent).context("failed to create Claude directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).context("failed to create Claude script")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))
            .context("failed to set Claude script permissions")?;
    }
    file.write_all(NO_PROXY_SCRIPT.as_bytes())
        .context("failed to write Claude script")?;
    file.as_file()
        .sync_all()
        .context("failed to sync Claude script")?;
    // Never overwrite a file that appeared while creating our script.
    file.persist_noclobber(path)
        .context("failed to save Claude script")?;
    Ok(())
}

fn remove_no_proxy_script(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to inspect Claude script"),
    };
    if metadata.is_file()
        && !metadata.file_type().is_symlink()
        && fs::read(path).context("failed to read Claude script")? == NO_PROXY_SCRIPT.as_bytes()
    {
        fs::remove_file(path).context("failed to remove Claude script")?;
    }
    Ok(())
}
