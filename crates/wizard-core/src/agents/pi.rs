use core::time::Duration;
use std::{
    fs,
    io::Write as _,
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
use crate::platform::{append_command_path, command_output, env_var_not_empty, find_executable};

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
    let settings = read_json(&dir.join("settings.json"))?;
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
    let settings = read_json(&dir.join("settings.json"))?;
    let original_auth = read_json(&dir.join("auth.json"))?;
    let installed = has_hub_package(&settings)?;
    ensure!(
        !dir.join("auth.json.lock").try_exists()?,
        "Pi auth is locked; close Pi and retry"
    );
    if install || installed {
        run_package_command(&dir, if install { "install" } else { "remove" })?;
    }
    let mut auth = original_auth.clone();
    let object = auth.as_object_mut().context("Pi auth must be an object")?;
    if install {
        object.insert(
            "coreinfra".to_owned(),
            json!({"type": "api_key", "key": token}),
        );
    } else {
        object.remove("coreinfra");
    }
    write_auth(&dir.join("auth.json"), &original_auth, &auth)
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

fn read_json(path: &Path) -> Result<Value> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            !metadata.file_type().is_symlink(),
            "Pi JSON file is a symlink; refusing to use it"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error).context("failed to inspect Pi JSON file"),
    }
    let bytes = fs::read(path).context("failed to read Pi JSON file")?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid Pi JSON file"))?;
    ensure!(value.is_object(), "Pi JSON file must contain an object");
    Ok(value)
}

fn write_auth(path: &Path, original: &Value, auth: &Value) -> Result<()> {
    if auth == original {
        return Ok(());
    }
    let parent = path.parent().context("Pi auth has no parent directory")?;
    fs::create_dir_all(parent).context("failed to create Pi agent directory")?;
    // Match proper-lockfile's lock-directory convention; never break an existing lock.
    let lock = path.with_file_name("auth.json.lock");
    fs::create_dir(&lock).context("Pi auth is locked; close Pi and retry")?;
    let result = (|| -> Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .context("failed to create temporary Pi auth file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .context("failed to restrict Pi auth permissions")?;
        }
        serde_json::to_writer_pretty(&mut temporary, auth)
            .map_err(|_| anyhow::anyhow!("failed to serialize Pi auth"))?;
        temporary
            .write_all(b"\n")
            .context("failed to write Pi auth")?;
        temporary
            .as_file()
            .sync_all()
            .context("failed to sync Pi auth")?;
        ensure!(
            &read_json(path)? == original,
            "Pi auth changed during the operation; close Pi and retry"
        );
        temporary.persist(path).context("failed to save Pi auth")?;
        Ok(())
    })();
    let unlock = fs::remove_dir(lock).context("failed to release Pi auth lock");
    result?;
    unlock
}
