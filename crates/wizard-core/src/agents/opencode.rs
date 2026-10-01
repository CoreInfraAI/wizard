use core::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};

use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    AgentDetection,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files,
    platform::{append_command_path, command_output, env_var_not_empty, find_executable},
};

const PACKAGE: &str = "@coreinfra/opencode-plugin";
const INSTALL_SOURCE: &str = "@coreinfra/opencode-plugin@latest";
static HUB_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct OpenCode {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub proxy_installed: bool,
}

pub(super) fn detect() -> AgentDetection<OpenCode> {
    let info = match detection::detect(Agent::OpenCode) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let proxy_installed = match proxy_installed() {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(OpenCode {
        info,
        proxy_installed,
    })
}

// OpenCode uses XDG directories on all platforms, including macOS and Windows.
fn config_dir() -> Result<PathBuf> {
    if let Some(base) = env_var_not_empty("XDG_CONFIG_HOME") {
        return std::path::absolute(PathBuf::from(base).join("opencode"))
            .context("failed to resolve OpenCode config directory");
    }
    let home = dirs::home_dir().context("failed to resolve OpenCode home directory")?;
    std::path::absolute(home.join(".config/opencode"))
        .context("failed to resolve OpenCode config directory")
}

fn auth_path() -> Result<PathBuf> {
    if let Some(base) = env_var_not_empty("XDG_DATA_HOME") {
        return std::path::absolute(PathBuf::from(base).join("opencode/auth.json"))
            .context("failed to resolve OpenCode auth path");
    }
    let home = dirs::home_dir().context("failed to resolve OpenCode home directory")?;
    std::path::absolute(home.join(".local/share/opencode/auth.json"))
        .context("failed to resolve OpenCode auth path")
}

fn read_configs(dir: &Path) -> Result<Vec<(PathBuf, Value)>> {
    // OpenCode merges all three global files. Only plain JSON is supported here.
    let mut configs = Vec::new();
    for name in ["config.json", "opencode.json", "opencode.jsonc"] {
        let path = dir.join(name);
        let value = config_files::json::read(&path)
            .with_context(|| format!("failed to read OpenCode {name}; Wizard currently supports JSON without comments or trailing commas"))?;
        if let Some(plugins) = value.get("plugin") {
            ensure!(plugins.is_array(), "OpenCode plugin must be an array");
        }
        configs.push((path, value));
    }
    Ok(configs)
}

fn is_hub_plugin(entry: &Value) -> bool {
    let source = entry
        .as_str()
        .or_else(|| entry.as_array()?.first()?.as_str());
    source.is_some_and(|source| {
        source == PACKAGE
            || source
                .strip_prefix(PACKAGE)
                .is_some_and(|suffix| suffix.starts_with('@'))
    })
}

fn has_hub_plugin(config: &Value) -> bool {
    config
        .get("plugin")
        .and_then(Value::as_array)
        .is_some_and(|plugins| plugins.iter().any(is_hub_plugin))
}

fn proxy_installed() -> Result<bool> {
    let _guard = HUB_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("OpenCode Hub lock poisoned"))?;
    let dir = config_dir()?;
    Ok(read_configs(&dir)?
        .iter()
        .any(|(_, config)| has_hub_plugin(config)))
}

/// Call from a Tokio blocking worker. Changes global configuration only; close `OpenCode` first.
pub fn set_hub(install: bool, token: &str) -> Result<()> {
    let _guard = HUB_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("OpenCode Hub lock poisoned"))?;
    let dir = config_dir()?;
    let auth_path = auth_path()?;
    let configs = read_configs(&dir)?;
    let original_auth = config_files::json::read(&auth_path)?;
    if install {
        run_install_command(&dir)?;
        ensure!(
            read_configs(&dir)?
                .iter()
                .any(|(_, config)| has_hub_plugin(config)),
            "OpenCode plugin command completed but no CoreInfra declaration was found"
        );
    } else {
        remove_plugin(&configs)?;
    }

    let mut auth = original_auth.clone();
    let credentials = auth
        .as_object_mut()
        .context("OpenCode auth must be an object")?;
    if install {
        credentials.insert("coreinfra".into(), json!({"type": "api", "key": token}));
    } else {
        credentials.remove("coreinfra");
    }
    config_files::json::write(&auth_path, &original_auth, &auth)
        .context("OpenCode plugin step completed, but saving authentication failed; close OpenCode and retry")
}

fn remove_plugin(configs: &[(PathBuf, Value)]) -> Result<()> {
    for (path, original) in configs {
        let mut config = original.clone();
        if let Some(plugins) = config.get_mut("plugin").and_then(Value::as_array_mut) {
            plugins.retain(|entry| !is_hub_plugin(entry));
        }
        config_files::json::write(path, original, &config)
            .context("failed to remove OpenCode Hub declaration; retry to finish cleanup")?;
    }
    Ok(())
}

fn run_install_command(config_dir: &Path) -> Result<()> {
    let opencode = find_executable("opencode", vec![PathBuf::from("~/.opencode/bin")])?
        .context("OpenCode executable not found")?;
    let cwd = tempfile::tempdir().context("failed to create OpenCode install working directory")?;
    let mut command = tokio::process::Command::new(opencode);
    if let Some(node) = find_executable("node", vec![])? {
        append_command_path(
            &mut command,
            &[node.parent().context("Node executable has no parent")?],
        )?;
    }
    command
        .args(["--pure", "plugin", "-g", "--force", INSTALL_SOURCE])
        .env(
            "XDG_CONFIG_HOME",
            config_dir
                .parent()
                .context("OpenCode config directory has no parent")?,
        )
        // This action manages global config, not overrides or project configuration.
        .env_remove("OPENCODE_CONFIG")
        .env_remove("OPENCODE_CONFIG_DIR")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .env_remove("OPENCODE_AUTH_CONTENT")
        .current_dir(cwd.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let output = command_output(command, Duration::from_secs(180))?;
    ensure!(
        output.status.success(),
        "OpenCode plugin installation failed with {}; settings may be partially updated; run the command manually for diagnostics",
        output.status
    );
    Ok(())
}
