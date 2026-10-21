use super::{
    AgentDetection, AgentToken, HubMode, auth_token,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files::{self, changes::FileChange},
    platform::{
        append_command_path, command_output, command_output_with_stdin, env_var_not_empty,
        find_executable,
    },
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
    settings::Settings,
};
use anyhow::{Context as _, Result, bail, ensure};
use core::time::Duration;
use jsonc_parser::cst::CstObject;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};

const PACKAGE: &str = "@coreinfra/opencode-plugin";
const INSTALL_SOURCE: &str = "@coreinfra/opencode-plugin@latest";
// OpenCode 2 keeps keys in its database. Wizard manages only its own key with this ID.
const CREDENTIAL_ID: &str = "cred_wizard";
const CREDENTIAL_PARAM: &str = "credentialID=cred_wizard";
const INTEGRATION: &str = "coreinfra";
// OpenCode 1 uses `plugin`; OpenCode 2 writes `plugins` and still reads `plugin`.
const PLUGIN_KEYS: [&str; 2] = ["plugin", "plugins"];
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
    let (routing_mode, token) = match read_routing(&info) {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(OpenCode {
        info,
        routing_mode,
        token,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Generation {
    /// Keys in `auth.json`.
    V1,
    /// Keys in the database, managed through the CLI.
    V2,
}

fn generation(info: &AgentInfo) -> Result<Generation> {
    let version = info
        .version
        .as_deref()
        .context("OpenCode version is unknown")?;
    let major: u32 = version
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .with_context(|| format!("unexpected OpenCode version {version}"))?;
    Ok(if major >= 2 {
        Generation::V2
    } else {
        Generation::V1
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

/// Global files `OpenCode` merges; both versions accept comments and trailing commas.
const fn config_names(generation: Generation) -> &'static [&'static str] {
    match generation {
        Generation::V1 => &["config.json", "opencode.json", "opencode.jsonc"],
        Generation::V2 => &["opencode.json", "opencode.jsonc"],
    }
}

fn read_configs(dir: &Path, generation: Generation) -> Result<(Vec<FileChange>, bool)> {
    let mut configs = Vec::new();
    let mut installed = false;
    for name in config_names(generation) {
        let change = FileChange::read(dir.join(name))?;

        let value = config_files::json::parse_jsonc(&change.after)
            .with_context(|| format!("failed to read OpenCode {name}"))?;
        for key in PLUGIN_KEYS {
            if let Some(plugins) = value.get(key) {
                ensure!(plugins.is_array(), "OpenCode {key} must be an array");
            }
        }
        installed |= has_hub_plugin(&value);
        configs.push(change);
    }
    Ok((configs, installed))
}

fn is_hub_plugin(entry: &Value) -> bool {
    // `"name"`, `["name", options]` or, in OpenCode 2, `{"package": "name", ...}`.
    let source = entry
        .as_str()
        .or_else(|| entry.as_array()?.first()?.as_str())
        .or_else(|| entry.get("package")?.as_str());
    source.is_some_and(|source| {
        source == PACKAGE
            || source
                .strip_prefix(PACKAGE)
                .is_some_and(|suffix| suffix.starts_with('@'))
    })
}

fn has_hub_plugin(config: &Value) -> bool {
    PLUGIN_KEYS.iter().any(|key| {
        config
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|plugins| plugins.iter().any(is_hub_plugin))
    })
}

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
/// Call from a Tokio blocking worker: `OpenCode` 2 is queried through its CLI.
/// Uses the same saved path as `prepare_hub`, so both see the same `OpenCode` generation.
pub fn configured_token(settings: &Settings) -> Result<Option<String>> {
    let info = match detection::detect(Agent::OpenCode, settings.last_paths.opencode.as_deref()) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return Ok(None),
        AgentDetection::Error(error) => bail!(error),
    };
    Ok(read_routing(&info)?.1.map(AgentToken::into_secret))
}

fn read_routing(info: &AgentInfo) -> Result<(HubMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::OpenCode.lock()?;
    let dir = config_dir()?;
    let generation = generation(info)?;
    let (_, installed) = read_configs(&dir, generation)?;
    let token = match generation {
        Generation::V1 => auth_token(&config_files::json::read(&auth_path()?)?),
        Generation::V2 => export_credential(&info.path)?
            .filter(|credential| credential.integration_id == INTEGRATION)
            .and_then(Credential::into_token),
    };
    let mode = if installed {
        HubMode::Hub
    } else {
        HubMode::Disabled
    };
    Ok((mode, token))
}

/// Prepare global configuration changes without running the installer or writing files.
/// Call from a Tokio blocking worker: detection runs the `OpenCode` CLI.
pub fn prepare_hub(mode: HubMode, token: &str, settings: &Settings) -> Result<PreparedPlan> {
    let info = match detection::detect(Agent::OpenCode, settings.last_paths.opencode.as_deref()) {
        AgentDetection::Found(info) => Some(info),
        // Without the CLI, disabling can still remove the plugin and the `auth.json` key.
        AgentDetection::NotFound if mode == HubMode::Disabled => None,
        AgentDetection::NotFound => bail!("OpenCode executable not found"),
        AgentDetection::Error(error) => bail!(error),
    };
    let generation = match &info {
        Some(info) => generation(info)?,
        None => Generation::V1,
    };

    let _guard = PreparedAgent::OpenCode.lock()?;

    let dir = config_dir()?;

    let (mut configs, _) = read_configs(&dir, generation)?;

    let mut steps = Vec::new();
    if mode == HubMode::Hub {
        let executable = info
            .as_ref()
            .context("OpenCode executable not found")?
            .path
            .clone();

        steps.push(match generation {
            Generation::V1 => PreparedStep::OpenCodeInstallPlugin {
                executable,
                config_dir: dir.clone(),
            },
            Generation::V2 => PreparedStep::OpenCodeAddPlugin {
                executable,
                config_dir: dir.clone(),
            },
        });
        steps.push(PreparedStep::OpenCodeVerifyPlugin { config_dir: dir });
        // Do not write unchanged config snapshots over the installer's output.
    } else {
        remove_plugin(&mut configs)?;
    }

    if generation == Generation::V1 {
        let mut auth = FileChange::read(auth_path()?)?;
        config_files::json::update(&mut auth.after, |auth| {
            let credentials = auth
                .as_object_mut()
                .context("OpenCode auth must be an object")?;
            if mode == HubMode::Hub {
                credentials.insert(INTEGRATION.into(), json!({"type": "api", "key": token}));
            } else {
                credentials.remove(INTEGRATION);
            }
            Ok(())
        })?;
        configs.push(auth);
    }

    steps.extend(
        configs
            .into_iter()
            .map(|change| PreparedStep::ChangeFile { change }),
    );

    if generation == Generation::V2 {
        let executable = info.context("OpenCode executable not found")?.path;
        steps.push(if mode == HubMode::Hub {
            PreparedStep::OpenCodeSetCredential {
                executable: executable.clone(),
                token: token.to_owned(),
            }
        } else {
            PreparedStep::OpenCodeRemoveCredential {
                executable: executable.clone(),
            }
        });
        // The running service caches configuration.
        steps.push(PreparedStep::OpenCodeReload { executable });
    }
    Ok(PreparedPlan::new(PreparedAgent::OpenCode, steps))
}

pub(crate) fn verify_plugin(config_dir: &Path) -> Result<()> {
    // Version 1's files include version 2's, so this accepts either installer's output.
    let (_, installed) = read_configs(config_dir, Generation::V1)?;
    ensure!(
        installed,
        "OpenCode plugin command completed but no CoreInfra declaration was found"
    );
    Ok(())
}

fn remove_plugin(configs: &mut [FileChange]) -> Result<()> {
    for change in configs {
        config_files::json::update_jsonc(&mut change.after, |config| {
            remove_hub_plugins(config);
            Ok(())
        })?;
    }
    Ok(())
}

fn remove_hub_plugins(config: &CstObject) {
    for key in PLUGIN_KEYS {
        let Some(plugins) = config.array_value(key) else {
            continue;
        };
        for entry in plugins.elements() {
            if entry
                .to_serde_value()
                .is_some_and(|entry| is_hub_plugin(&entry))
            {
                entry.remove();
            }
        }
    }
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

// No Debug: the value contains the key.
#[derive(Deserialize)]
struct Credential {
    id: String,
    #[serde(rename = "integrationID")]
    integration_id: String,
    active: bool,
    value: Value,
}

impl Credential {
    fn key(&self) -> Option<&str> {
        (self.value["type"] == "key")
            .then(|| self.value["key"].as_str())
            .flatten()
    }

    fn into_token(self) -> Option<AgentToken> {
        AgentToken::new(self.key()?.to_owned())
    }
}

/// The key with Wizard's ID from `auth export`, which prints every stored key, whatever its
/// integration. Never log the output.
fn export_credential(executable: &Path) -> Result<Option<Credential>> {
    let output = run_cli(
        executable,
        "auth export",
        &["auth", "export", "--standalone"],
        None,
    )?;
    // Parse errors may quote the input, so do not attach them.
    let credentials: Vec<Credential> = serde_json::from_slice(&output)
        .map_err(|_| anyhow::anyhow!("unexpected OpenCode auth export output"))?;
    Ok(credentials
        .into_iter()
        .find(|credential| credential.id == CREDENTIAL_ID))
}

/// Call from a Tokio blocking worker. Replaces Wizard's key and makes it active;
/// other `CoreInfra` keys stay unchanged.
pub(crate) fn set_credential(executable: &Path, token: &str) -> Result<()> {
    // Import skips an existing ID, and OpenCode cannot replace a stored key.
    remove_credential(executable)?;

    let input = serde_json::to_vec(&json!([{
        "id": CREDENTIAL_ID,
        "integrationID": INTEGRATION,
        "label": "Wizard",
        "active": true,
        "value": {"type": "key", "key": token},
    }]))?;
    run_cli(
        executable,
        "auth import",
        &["auth", "import", "--standalone"],
        Some(&input),
    )?;
    // Import activates a key only when the integration has no active key.
    run_cli(
        executable,
        "credential.activate",
        &[
            "api",
            "--standalone",
            "credential.activate",
            "--param",
            CREDENTIAL_PARAM,
        ],
        None,
    )?;

    // The commands above also succeed for a missing ID.
    let saved = export_credential(executable)?.is_some_and(|credential| {
        credential.integration_id == INTEGRATION
            && credential.active
            && credential.key() == Some(token)
    });
    ensure!(saved, "OpenCode did not save the active CoreInfra key");
    Ok(())
}

/// Call from a Tokio blocking worker. Removes only Wizard's key; when it was active,
/// `OpenCode` activates another remaining key.
/// Fails without changes if the ID belongs to another integration.
pub(crate) fn remove_credential(executable: &Path) -> Result<()> {
    let Some(credential) = export_credential(executable)? else {
        return Ok(());
    };
    ensure!(
        credential.integration_id == INTEGRATION,
        "OpenCode key {CREDENTIAL_ID} belongs to another integration; Wizard will not change it"
    );

    run_cli(
        executable,
        "credential.remove",
        &[
            "api",
            "--standalone",
            "credential.remove",
            "--param",
            CREDENTIAL_PARAM,
        ],
        None,
    )?;
    ensure!(
        export_credential(executable)?.is_none(),
        "OpenCode did not remove the CoreInfra key"
    );
    Ok(())
}

/// Call from a Tokio blocking worker. Reloads the configuration of `OpenCode` 2's background
/// service if it is running; `reload` would start a stopped service.
pub(crate) fn reload(executable: &Path) -> Result<()> {
    let status = run_cli(executable, "service status", &["service", "status"], None)?;
    if status.trim_ascii() == b"stopped" {
        log::info!("OpenCode service is stopped; reload skipped");
        return Ok(());
    }
    run_cli(executable, "reload", &["reload"], None)?;
    Ok(())
}

/// Runs an `OpenCode` 2 command that needs no network and returns its stdout.
/// Discards stderr because it may echo keys; `operation` is the static name for errors.
fn run_cli(
    executable: &Path,
    operation: &str,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let cwd = tempfile::tempdir().context("failed to create OpenCode working directory")?;

    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .env_remove("OPENCODE_CONFIG")
        .env_remove("OPENCODE_CONFIG_DIR")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .env_remove("OPENCODE_AUTH_CONTENT")
        .current_dir(cwd.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // The CLI talks HTTP to its local server or service; a proxy answers 404 instead.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }

    let timeout = Duration::from_secs(30);
    let output = match input {
        Some(input) => command_output_with_stdin(command, input, timeout)?,
        None => command_output(command, timeout)?,
    };
    ensure!(
        output.status.success(),
        "OpenCode {operation} failed with {}",
        output.status
    );
    Ok(output.stdout)
}

/// Call from a Tokio blocking worker. Runs `plugin add` for `OpenCode` 2; does not set the key.
/// The command may start or use `OpenCode`'s background service.
pub fn add_plugin(executable: &Path, config_dir: &Path) -> Result<()> {
    let cwd = tempfile::tempdir().context("failed to create OpenCode install working directory")?;

    let mut command = tokio::process::Command::new(executable);
    command
        .args(["plugin", "add", INSTALL_SOURCE])
        .env(
            "XDG_CONFIG_HOME",
            config_dir
                .parent()
                .context("OpenCode config directory has no parent")?,
        )
        .env_remove("OPENCODE_CONFIG")
        .env_remove("OPENCODE_CONFIG_DIR")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .env_remove("OPENCODE_AUTH_CONTENT")
        .current_dir(cwd.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The package download may need a proxy, but the CLI's calls to its local service must not
    // use it.
    for name in ["NO_PROXY", "no_proxy"] {
        let mut hosts = std::env::var_os(name).unwrap_or_default();
        if !hosts.is_empty() {
            hosts.push(",");
        }
        hosts.push("127.0.0.1,localhost");
        command.env(name, hosts);
    }

    let output = command_output(command, Duration::from_secs(180))?;
    ensure!(
        output.status.success(),
        "OpenCode plugin installation failed with {}; run `opencode plugin add {INSTALL_SOURCE}` manually for diagnostics",
        output.status
    );
    Ok(())
}
