use core::time::Duration;
use std::{
    fs,
    path::{Path, PathBuf},
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

const PACKAGE: &str = "npm:@coreinfra/pi-plugin";
const INSTALL_SOURCE: &str = "npm:@coreinfra/pi-plugin@latest";
static HUB_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Pi {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub proxy_installed: bool,
}

pub(super) fn detect() -> AgentDetection<Pi> {
    let info = match detection::detect(Agent::Pi) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let proxy_installed = match proxy_installed() {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Pi {
        info,
        proxy_installed,
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

fn proxy_installed() -> Result<bool> {
    let _guard = HUB_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Pi Hub lock poisoned"))?;
    let dir = agent_dir()?;
    let settings = config_files::json::read(&dir.join("settings.json"))?;
    has_hub_package(&settings)
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

/// Call from a Tokio blocking worker. Close Pi before changing its configuration.
pub fn set_hub(install: bool, token: &str) -> Result<()> {
    let _guard = HUB_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Pi Hub lock poisoned"))?;
    let dir = agent_dir()?;
    let settings = config_files::json::read(&dir.join("settings.json"))?;

    let mut auth = FileChange::read(dir.join("auth.json"))?;

    let installed = has_hub_package(&settings)?;
    config_files::json::update(&mut auth.after, |auth| {
        let object = auth.as_object_mut().context("Pi auth must be an object")?;
        if install {
            object.insert(
                "coreinfra".to_owned(),
                json!({"type": "api_key", "key": token}),
            );
        } else {
            object.remove("coreinfra");
        }
        Ok(())
    })?;
    ensure!(
        !dir.join("auth.json.lock").try_exists()?,
        "Pi auth is locked; close Pi and retry"
    );
    if install || installed {
        run_package_command(&dir, if install { "install" } else { "remove" })?;
    }
    apply_auth(&dir, auth)
        .context("Pi package step completed, but saving authentication failed; close Pi and retry")
}

fn run_package_command(dir: &Path, action: &str) -> Result<()> {
    let extra = vec![PathBuf::from("~/.pi/agent/bin")];
    let pi = find_executable("pi", extra)?.context("Pi executable not found")?;
    let node = find_executable("node", vec![])?.context("Node executable not found")?;
    let npm = find_executable("npm", vec![])?.context("npm executable not found")?;
    let paths = [
        node.parent().context("Node executable has no parent")?,
        npm.parent().context("npm executable has no parent")?,
    ];
    // Never run package commands in a user's project directory.
    let cwd = tempfile::tempdir().context("failed to create Pi package working directory")?;
    let mut command = tokio::process::Command::new(pi);
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

fn apply_auth(dir: &Path, auth: FileChange) -> Result<()> {
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
