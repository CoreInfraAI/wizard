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
    config_files::{self, changes::FileChange},
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

fn read_configs(dir: &Path) -> Result<(Vec<FileChange>, bool)> {
    // OpenCode merges all three global files. Only plain JSON is supported here.
    let mut configs = Vec::new();
    let mut installed = false;
    for name in ["config.json", "opencode.json", "opencode.jsonc"] {
        let change = FileChange::read(dir.join(name))?;
        let value = config_files::json::parse(change.after.as_ref())
            .with_context(|| format!("failed to read OpenCode {name}; Wizard currently supports JSON without comments or trailing commas"))?;
        if let Some(plugins) = value.get("plugin") {
            ensure!(plugins.is_array(), "OpenCode plugin must be an array");
        }
        installed |= has_hub_plugin(&value);
        configs.push(change);
    }
    Ok((configs, installed))
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
    let (_, installed) = read_configs(&dir)?;
    Ok(installed)
}

/// Call from a Tokio blocking worker. Changes global configuration only; close `OpenCode` first.
pub fn set_hub(install: bool, token: &str) -> Result<()> {
    let _guard = HUB_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("OpenCode Hub lock poisoned"))?;
    let dir = config_dir()?;
    let (mut configs, _) = read_configs(&dir)?;
    let mut auth = FileChange::read(auth_path()?)?;
    config_files::json::update(&mut auth.after, |auth| {
        let credentials = auth
            .as_object_mut()
            .context("OpenCode auth must be an object")?;
        if install {
            credentials.insert("coreinfra".into(), json!({"type": "api", "key": token}));
        } else {
            credentials.remove("coreinfra");
        }
        Ok(())
    })?;
    if install {
        run_install_command(&dir)?;
        let (_, installed) = read_configs(&dir)?;
        ensure!(
            installed,
            "OpenCode plugin command completed but no CoreInfra declaration was found"
        );
        // The installer owns config edits. Their unchanged snapshots are skipped
        // by apply_all, so its output is never replaced by pre-install contents.
    } else {
        remove_plugin(&mut configs)?;
    }

    configs.push(auth);
    FileChange::apply_all(&configs).context(if install {
        "OpenCode plugin installation completed, but saving authentication failed; close OpenCode and retry"
    } else {
        "failed to apply OpenCode Hub removal; close OpenCode and retry"
    })
}

fn remove_plugin(configs: &mut [FileChange]) -> Result<()> {
    for change in configs {
        config_files::json::update(&mut change.after, |config| {
            if let Some(plugins) = config.get_mut("plugin").and_then(Value::as_array_mut) {
                plugins.retain(|entry| !is_hub_plugin(entry));
            }
            Ok(())
        })?;
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
