use super::{
    AgentDetection, AgentToken, HubMode, auth_token,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files::{self, changes::FileChange},
    platform::{append_command_path, command_output, env_var_not_empty, find_executable},
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
    settings::Settings,
};
use anyhow::{Context as _, Result, ensure};
use core::time::Duration;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};

const PACKAGE: &str = "@coreinfra/opencode-plugin";
const INSTALL_SOURCE: &str = "@coreinfra/opencode-plugin@latest";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct OpenCode {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: HubMode,
    pub token: Option<AgentToken>,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<OpenCode> {
    let info = match detection::detect(Agent::OpenCode, settings.last_paths.opencode.as_deref()) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let (routing_mode, token) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(OpenCode {
        info,
        routing_mode,
        token,
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

        let value = config_files::json::parse(&change.after)
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

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

fn read_routing() -> Result<(HubMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::OpenCode.lock()?;
    let dir = config_dir()?;
    let (_, installed) = read_configs(&dir)?;
    let auth = config_files::json::read(&auth_path()?)?;
    let mode = if installed {
        HubMode::Hub
    } else {
        HubMode::Disabled
    };
    Ok((mode, auth_token(&auth)))
}

/// Prepare global configuration changes without running the installer or writing files.
pub fn prepare_hub(mode: HubMode, token: &str, settings: &Settings) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::OpenCode.lock()?;

    let dir = config_dir()?;

    let (mut configs, _) = read_configs(&dir)?;
    let mut auth = FileChange::read(auth_path()?)?;

    config_files::json::update(&mut auth.after, |auth| {
        let credentials = auth
            .as_object_mut()
            .context("OpenCode auth must be an object")?;
        if mode == HubMode::Hub {
            credentials.insert("coreinfra".into(), json!({"type": "api", "key": token}));
        } else {
            credentials.remove("coreinfra");
        }
        Ok(())
    })?;

    let mut steps = Vec::new();
    if mode == HubMode::Hub {
        let executable = find_executable(
            "opencode",
            vec![PathBuf::from("~/.opencode/bin")],
            settings.last_paths.opencode.as_deref(),
        )?
        .context("OpenCode executable not found")?;

        steps.push(PreparedStep::OpenCodeInstallPlugin {
            executable,
            config_dir: dir.clone(),
        });
        steps.push(PreparedStep::OpenCodeVerifyPlugin { config_dir: dir });
        // Do not write unchanged config snapshots over the installer's output.
    } else {
        remove_plugin(&mut configs)?;
    }

    configs.push(auth);
    steps.extend(
        configs
            .into_iter()
            .map(|change| PreparedStep::ChangeFile { change }),
    );
    Ok(PreparedPlan::new(PreparedAgent::OpenCode, steps))
}

pub(crate) fn verify_plugin(config_dir: &Path) -> Result<()> {
    let (_, installed) = read_configs(config_dir)?;
    ensure!(
        installed,
        "OpenCode plugin command completed but no CoreInfra declaration was found"
    );
    Ok(())
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

/// Call from a Tokio blocking worker. Only runs the installer; does not update auth.
pub fn install_plugin(executable: &Path, config_dir: &Path) -> Result<()> {
    let cwd = tempfile::tempdir().context("failed to create OpenCode install working directory")?;

    let mut command = tokio::process::Command::new(executable);
    if let Some(node) = find_executable("node", vec![], None)? {
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
