use super::{
    AgentDetection, AgentToken, HubMode, auth_token,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files::{self, changes::FileChange},
    platform::{append_command_path, command_output, env_var_not_empty, find_executable},
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result, ensure};
use core::time::Duration;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use wizard_settings::Settings;

const PACKAGE: &str = "npm:@coreinfra/pi-plugin";
const INSTALL_SOURCE: &str = "npm:@coreinfra/pi-plugin@latest";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Pi {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: HubMode,
    pub token: Option<AgentToken>,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<Pi> {
    let info = match detection::detect(Agent::Pi, settings.last_paths.pi.as_deref()) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let (routing_mode, token) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Pi {
        info,
        routing_mode,
        token,
    })
}

fn agent_dir() -> Result<PathBuf> {
    if let Some(path) = env_var_not_empty("PI_CODING_AGENT_DIR") {
        let path = PathBuf::from(path);
        if let Ok(relative) = path.strip_prefix("~") {
            return Ok(dirs::home_dir()
                .context("failed to resolve Pi home directory")?
                .join(relative));
        }
        return std::path::absolute(path).context("failed to resolve Pi agent directory");
    }
    Ok(dirs::home_dir()
        .context("failed to resolve Pi home directory")?
        .join(".pi/agent"))
}

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

fn read_routing() -> Result<(HubMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::Pi.lock()?;
    let dir = agent_dir()?;
    let settings = config_files::json::read(&dir.join("settings.json"))?;
    let auth = config_files::json::read(&dir.join("auth.json"))?;
    let mode = if has_hub_package(&settings)? {
        HubMode::Hub
    } else {
        HubMode::Disabled
    };
    Ok((mode, auth_token(&auth)))
}

fn has_hub_package(settings: &Value) -> Result<bool> {
    let Some(packages) = settings.get("packages") else {
        return Ok(false);
    };
    let packages = packages
        .as_array()
        .context("Pi packages must be an array")?;
    let mut found = false;
    for entry in packages {
        let source = entry
            .as_str()
            .or_else(|| entry.get("source").and_then(Value::as_str))
            .context("invalid Pi package declaration")?;
        if source == PACKAGE
            || source
                .strip_prefix(PACKAGE)
                .is_some_and(|suffix| suffix.starts_with('@'))
        {
            found = true;
        }
    }
    Ok(found)
}

/// Prepare without running the package manager or writing authentication.
pub fn prepare_hub(mode: HubMode, token: &str, settings: &Settings) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::Pi.lock()?;

    let dir = std::path::absolute(agent_dir()?)?;
    let pi_settings = config_files::json::read(&dir.join("settings.json"))?;

    let mut auth = FileChange::read(dir.join("auth.json"))?;

    let installed = has_hub_package(&pi_settings)?;
    config_files::json::update(&mut auth.after, |auth| {
        let object = auth.as_object_mut().context("Pi auth must be an object")?;
        if mode == HubMode::Hub {
            object.insert(
                "coreinfra".to_owned(),
                json!({"type": "api_key", "key": token}),
            );
        } else {
            object.shift_remove("coreinfra");
        }
        Ok(())
    })?;

    ensure!(
        !dir.join("auth.json.lock").try_exists()?,
        "Pi auth is locked; close Pi and retry"
    );

    let mut steps = Vec::new();
    if mode == HubMode::Hub || installed {
        let executable = find_executable(
            "pi",
            vec![PathBuf::from("~/.pi/agent/bin")],
            settings.last_paths.pi.as_deref(),
        )?
        .context("Pi executable not found")?;

        steps.push(if mode == HubMode::Hub {
            PreparedStep::PiInstallPlugin {
                executable,
                agent_dir: dir.clone(),
            }
        } else {
            PreparedStep::PiRemovePlugin {
                executable,
                agent_dir: dir.clone(),
            }
        });
    }

    if auth.is_changed() {
        steps.push(PreparedStep::PiApplyAuth { dir, change: auth });
    }
    Ok(PreparedPlan::new(PreparedAgent::Pi, steps))
}

/// Call from a Tokio blocking worker. Only installs the package; does not update auth.
pub fn install_plugin(executable: &Path, agent_dir: &Path) -> Result<()> {
    run_package_command(executable, agent_dir, "install")
}

/// Call from a Tokio blocking worker. Only removes the package; does not update auth.
pub fn remove_plugin(executable: &Path, agent_dir: &Path) -> Result<()> {
    run_package_command(executable, agent_dir, "remove")
}

fn run_package_command(executable: &Path, dir: &Path, action: &str) -> Result<()> {
    // Version managers (nvm, fnm, Volta) keep node and npm next to the Pi CLI.
    let near: Vec<_> = executable
        .parent()
        .map(Path::to_owned)
        .into_iter()
        .collect();
    let node = find_executable("node", near.clone(), None)?.context("Node executable not found")?;
    let npm = find_executable("npm", near, None)?.context("npm executable not found")?;
    let paths = [
        node.parent().context("Node executable has no parent")?,
        npm.parent().context("npm executable has no parent")?,
    ];

    // Never run package commands in a user's project directory.
    let cwd = tempfile::tempdir().context("failed to create Pi package working directory")?;

    let mut command = tokio::process::Command::new(executable);
    append_command_path(&mut command, &paths)?;
    command
        .args([action, INSTALL_SOURCE])
        .env("PI_CODING_AGENT_DIR", dir)
        .current_dir(cwd.path())
        // Package-manager output may include credentials from npm configuration.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let output = command_output(command, Duration::from_secs(60))?;
    ensure!(
        output.status.success(),
        "Pi package {action} failed with {}; configuration may be partially updated; run the Pi command manually for diagnostics",
        output.status
    );
    Ok(())
}

pub(crate) fn apply_auth(dir: &Path, auth: FileChange) -> Result<()> {
    if !auth.is_changed() {
        return Ok(());
    }
    fs::create_dir_all(dir).context("failed to create Pi agent directory")?;
    // Match proper-lockfile's lock-directory convention; never break an existing lock.
    let lock = dir.join("auth.json.lock");
    fs::create_dir(&lock).context("Pi auth is locked; close Pi and retry")?;
    let result = FileChange::apply_all(&[auth]);
    let unlock = fs::remove_dir(lock).context("failed to release Pi auth lock");
    result?;
    unlock
}
