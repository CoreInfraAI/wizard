use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, Agent, AgentInfo, DesktopAgent},
};
use crate::{
    config_files::{
        self,
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
    platform::{
        append_node_path, command_output, daemon_command_status, env_var_not_empty,
        find_executable, is_executable,
    },
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result, ensure};
use core::time::Duration;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
};
use toml_edit::DocumentMut;
use wizard_settings::Settings;

pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Codex {
    pub cli: AgentDetection<AgentInfo>,
    pub desktop: AgentDetection<AgentInfo>,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<Codex> {
    let cli = detection::detect(Agent::Codex, settings.last_paths.codex.as_deref());
    let desktop = detection::detect_desktop(
        DesktopAgent::Codex,
        settings.last_paths.codex_desktop.as_deref(),
    );
    if !matches!(cli, AgentDetection::Found(_)) && !matches!(desktop, AgentDetection::Found(_)) {
        let mut errors = Vec::new();
        if let AgentDetection::Error(error) = cli {
            errors.push(format!("Codex CLI: {error}"));
        }
        if let AgentDetection::Error(error) = desktop {
            errors.push(format!("ChatGPT: {error}"));
        }
        return if errors.is_empty() {
            AgentDetection::NotFound
        } else {
            AgentDetection::Error(errors.join("; "))
        };
    }
    let (routing_mode, token) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Codex {
        cli,
        desktop,
        routing_mode,
        token,
    })
}

fn config_path() -> Result<PathBuf> {
    if let Some(home) = env_var_not_empty("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = dirs::home_dir().context("failed to resolve home directory")?;
    Ok(home.join(".codex/config.toml"))
}

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

fn read_routing() -> Result<(RoutingMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::Codex.lock()?;
    let path = config_path()?;
    let doc = config_files::toml::read(&path)?;
    let dotenv = FileSnapshot::read(&path.with_file_name(".env"))?;
    let token = config_files::env::get(&dotenv, "COREINFRA_API_KEY")
        .context("failed to read Codex .env")?
        .and_then(AgentToken::new);
    Ok((mode_from_config(&doc), token))
}

fn mode_from_config(doc: &DocumentMut) -> RoutingMode {
    match config_files::toml::get_string(doc, &["model_provider"]) {
        Some("coreinfra") => RoutingMode::Hub,
        Some("coreinfra-subscription") => RoutingMode::Sub,
        _ => RoutingMode::Disabled,
    }
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::get_backups(AgentKind::Codex)
}

pub fn restore_backup(id: u32) -> Result<()> {
    let _guard = PreparedAgent::Codex.lock()?;
    BackupManager::load_backup(AgentKind::Codex, id)
}

/// Call from a Tokio blocking worker. Restart only if the version probe finds a managed daemon.
/// A restart may interrupt active work; this does not change routing configuration.
pub fn restart_daemon(executable: &Path, codex_home: &Path) -> Result<()> {
    if !managed_daemon_running(executable, codex_home) {
        log::info!("Codex daemon restart skipped: no managed daemon is running");
        return Ok(());
    }

    // Unlike `daemon restart`, bootstrap also replaces a running updater, which relaunches the
    // daemon on updates. It keeps variables later removed from `.env`, and one started by an
    // older Wizard stays attributed to Wizard by macOS. Bootstrap resets remote control, though.
    let mut command = tokio::process::Command::new(executable);
    append_node_path(&mut command)?;
    command
        .args(["app-server", "daemon", "bootstrap"])
        .env("CODEX_HOME", codex_home);
    if remote_control_enabled(codex_home)? {
        command.arg("--remote-control");
    }

    log::info!("Restarting Codex daemon");
    // Allow the daemon's maximum five-minute graceful shutdown, plus startup and updater readiness.
    let status = daemon_command_status(command, Duration::from_secs(420))
        .inspect_err(|_| {
            log::error!("Codex daemon restart failed: command could not complete");
        })
        .context(
            "Codex daemon restart failed; run `codex app-server daemon restart` in a terminal",
        )?;
    if !status.success() {
        log::error!("Codex daemon restart failed with {status}");
    }
    ensure!(
        status.success(),
        "Codex daemon restart failed with {status}; run `codex app-server daemon restart` in a terminal"
    );
    log::info!("Codex daemon restarted successfully");
    Ok(())
}

/// Whether `daemon version` reports an app-server that the daemon manages.
fn managed_daemon_running(executable: &Path, codex_home: &Path) -> bool {
    #[derive(Deserialize)]
    struct Version {
        backend: Option<String>,
    }

    let mut probe = tokio::process::Command::new(executable);
    probe
        .args(["app-server", "daemon", "version"])
        .env("CODEX_HOME", codex_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // TODO: Distinguish an absent daemon from command failures and unsupported versions.
    // For now, spawn errors, timeouts, nonzero exit statuses and unmanaged servers all mean
    // "no daemon".
    append_node_path(&mut probe)
        .and_then(|()| command_output(probe, Duration::from_secs(15)))
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Version>(&output.stdout).ok())
        .is_some_and(|version| version.backend.as_deref() == Some("pid"))
}

fn remote_control_enabled(codex_home: &Path) -> Result<bool> {
    let path = codex_home.join("app-server-daemon/settings.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let settings: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(settings
        .get("remoteControlEnabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

/// Prepare without writing files or probing the daemon. `token` is ignored when disabled.
pub fn prepare_routing(
    mode: RoutingMode,
    token: &str,
    settings: &Settings,
) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::Codex.lock()?;

    let path = std::path::absolute(config_path()?)?;
    let codex_home = path
        .parent()
        .context("Codex config has no parent")?
        .to_owned();

    let mut config = FileChange::read(path.clone())?;
    let mut dotenv = FileChange::read(path.with_file_name(".env"))?;

    let mut current_mode = RoutingMode::Disabled;
    config_files::toml::update(&mut config.after, |doc| {
        current_mode = mode_from_config(doc);
        remove_inactive_routing_settings(doc, current_mode)?;
        write_routing_settings(doc, mode)
    })?;

    remove_inactive_routing_env(&mut dotenv.after, current_mode, mode)?;
    write_routing_env(&mut dotenv.after, mode, token)?;

    Ok(PreparedPlan::new(
        PreparedAgent::Codex,
        vec![
            PreparedStep::ChangeFile { change: config },
            PreparedStep::ChangeFile { change: dotenv },
            PreparedStep::CodexRestartDaemon {
                executable: daemon_executable(settings)?,
                codex_home,
            },
        ],
    ))
}

/// Add GUI-owned proxy demand and `ChatGPT` profile configuration.
pub fn prepare_routing_for_gui(
    mode: RoutingMode,
    token: &str,
    settings: &Settings,
) -> Result<PreparedPlan> {
    let mut plan = prepare_routing(mode, token, settings)?;
    let enabled = if mode == RoutingMode::Disabled {
        false
    } else {
        match detection::detect_desktop(
            DesktopAgent::Codex,
            settings.last_paths.codex_desktop.as_deref(),
        ) {
            AgentDetection::Found(_) => true,
            // Clean up a configuration left from before ChatGPT was removed.
            AgentDetection::NotFound => false,
            AgentDetection::Error(error) => {
                anyhow::bail!("failed to detect ChatGPT for local proxy: {error}")
            }
        }
    };
    // Without ChatGPT installed (Windows), there is no profile to clean up when disabling.
    match desktop_preferences_path()? {
        Some(path) => {
            let change = prepare_desktop_proxy(path, enabled)?;
            plan.steps.insert(0, PreparedStep::ChangeFile { change });
        }
        None => ensure!(!enabled, "ChatGPT profile location is unavailable"),
    }
    // Before the daemon restart: it can fail after Preferences already points at the listener.
    let restart = plan
        .steps
        .iter()
        .position(|step| matches!(step, PreparedStep::CodexRestartDaemon { .. }))
        .unwrap_or(plan.steps.len());
    plan.steps.insert(
        restart,
        PreparedStep::SetCodexProxyEnabled {
            token: enabled.then(|| token.to_owned()),
        },
    );
    Ok(plan)
}

fn desktop_preferences_path() -> Result<Option<PathBuf>> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let config =
            dirs::config_dir().context("failed to resolve ChatGPT configuration directory")?;
        Ok(Some(config.join("Codex/Default/Preferences")))
    }
    #[cfg(target_os = "windows")]
    {
        // MSIX redirects roaming app data into the current user's package directory.
        // Query metadata only; do not launch the app or read its profile here.
        let script = r"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$package = Get-AppxPackage -Name OpenAI.Codex | Sort-Object Version -Descending | Select-Object -First 1
if ($null -eq $package) { 'null'; exit 0 }
$root = Join-Path (Join-Path $env:LOCALAPPDATA 'Packages') $package.PackageFamilyName
Join-Path $root 'LocalCache\Roaming\Codex\web\Codex\Default\Preferences' | ConvertTo-Json -Compress
";
        let mut command = tokio::process::Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command_output(command, Duration::from_secs(15))?;
        ensure!(
            output.status.success(),
            "ChatGPT profile location query failed"
        );
        serde_json::from_slice(&output.stdout).context("invalid ChatGPT profile location response")
    }
}

fn prepare_desktop_proxy(path: PathBuf, enabled: bool) -> Result<FileChange> {
    use crate::config_files::{changes::FileSnapshot, json};
    use serde_json::json;

    let mut change = FileChange::read(std::path::absolute(path)?)?;
    // ChatGPT creates its profile on first launch; until then there is nothing to clean up.
    if matches!(change.after, FileSnapshot::Missing) {
        // Linux detection accepts any `chatgpt` executable, which may be another program.
        ensure!(
            !enabled || cfg!(target_os = "linux"),
            "ChatGPT ещё не запускался: откройте его один раз и повторите."
        );
        log::info!("ChatGPT proxy configuration skipped: profile Preferences file is missing");
        return Ok(change);
    }

    let proxy = json!({
        "mode": "fixed_servers",
        "server": format!("http://{}", wizard_proxy::CHATGPT_ADDRESS),
    });
    // Written before ChatGPT had its own listener; replaced or removed like the current one.
    let shared_proxy = json!({
        "mode": "fixed_servers",
        "server": "http://127.0.0.1:33787",
    });
    json::update(&mut change.after, |config| {
        let current = config.get("proxy");
        let foreign = current.is_some_and(|current| current != &proxy && current != &shared_proxy);

        if enabled {
            if foreign {
                log::warn!("ChatGPT proxy configuration replaces a proxy not managed by Wizard");
            }
            config["proxy"] = proxy;
        } else if foreign {
            log::warn!("ChatGPT proxy removal skipped: existing proxy is not managed by Wizard");
        } else {
            config
                .as_object_mut()
                .context("ChatGPT Preferences must contain an object")?
                .shift_remove("proxy");
        }
        Ok(())
    })?;
    Ok(change)
}

/// Prefer the standalone CLI; only discover the desktop app when it is absent.
fn daemon_executable(settings: &Settings) -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let extra = vec![PathBuf::from("%LOCALAPPDATA%/Programs/OpenAI/Codex/bin")];
    #[cfg(not(target_os = "windows"))]
    let extra = vec![];
    if let Some(executable) = find_executable("codex", extra, settings.last_paths.codex.as_deref())?
    {
        return Ok(executable);
    }

    let desktop = match detection::detect_desktop(
        DesktopAgent::Codex,
        settings.last_paths.codex_desktop.as_deref(),
    ) {
        AgentDetection::Found(desktop) => desktop.path,
        AgentDetection::NotFound => anyhow::bail!("Codex CLI and ChatGPT not found"),
        AgentDetection::Error(error) => {
            anyhow::bail!("failed to find ChatGPT for daemon restart: {error}")
        }
    };
    #[cfg(target_os = "macos")]
    let resources = desktop.join("Contents/Resources");
    #[cfg(not(target_os = "macos"))]
    let resources = {
        // Linux launchers may be symlinks into the installation directory.
        let executable = std::fs::canonicalize(&desktop)
            .with_context(|| format!("failed to resolve {}", desktop.display()))?;
        executable
            .parent()
            .context("desktop executable has no parent")?
            .join("resources")
    };
    #[cfg(target_os = "windows")]
    let candidates = ["codex-cli/bin/codex.exe", "codex.exe"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["codex-cli/bin/codex", "codex"];

    for relative in candidates {
        let path = resources.join(relative);
        if is_executable(&path) {
            return std::path::absolute(path).context("failed to resolve bundled Codex executable");
        }
    }
    anyhow::bail!(
        "bundled Codex executable not found in {}",
        resources.display()
    )
}

const API_FILTERS: &[&str] = &[
    "COREINFRA_API_KEY",
    "COREINFRA_PROXY_URL",
    "HTTPS_PROXY",
    "NO_PROXY",
];

fn remove_inactive_routing_settings(
    doc: &mut DocumentMut,
    current_mode: RoutingMode,
) -> Result<()> {
    if current_mode != RoutingMode::Disabled {
        config_files::toml::remove(doc, &["model_provider"])?;
    }

    config_files::toml::remove(doc, &["model_providers", "coreinfra"])?;
    config_files::toml::remove(doc, &["model_providers", "coreinfra-subscription"])?;

    if current_mode == RoutingMode::Hub {
        config_files::toml::remove(doc, &["features", "api_key_model_discovery"])?;
    }
    if current_mode == RoutingMode::Sub {
        config_files::toml::remove(doc, &["features", "shell_snapshot"])?;
        for key in API_FILTERS {
            config_files::toml::remove(doc, &["shell_environment_policy", "filters", key])?;
        }
    }
    Ok(())
}

fn write_routing_settings(doc: &mut DocumentMut, mode: RoutingMode) -> Result<()> {
    let (provider, name, base_url) = match mode {
        RoutingMode::Disabled => return Ok(()),
        RoutingMode::Hub => (
            "coreinfra",
            "CoreInfra AI Hub",
            "https://hub.coreinfra.ai/codex/api/v1",
        ),
        RoutingMode::Sub => (
            "coreinfra-subscription",
            "CoreInfra AI Hub | ChatGPT",
            "https://hub.coreinfra.ai/openai/subscription/api/v1",
        ),
    };
    config_files::toml::set_value(doc, &["model_provider"], provider)?;
    for (key, value) in [
        ("name", name),
        ("base_url", base_url),
        ("wire_api", "responses"),
    ] {
        config_files::toml::set_value(doc, &["model_providers", provider, key], value)?;
    }

    match mode {
        RoutingMode::Hub => {
            config_files::toml::set_value(doc, &["features", "api_key_model_discovery"], true)?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "env_key"],
                "COREINFRA_API_KEY",
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "model_catalog_url"],
                "https://hub.coreinfra.ai/codex/api/v1/models",
            )?;
        }
        RoutingMode::Sub => {
            config_files::toml::set_value(doc, &["features", "shell_snapshot"], false)?;
            for key in API_FILTERS {
                config_files::toml::set_value(
                    doc,
                    &["shell_environment_policy", "filters", key],
                    "exclude",
                )?;
            }
            let mut headers = toml_edit::InlineTable::new();
            headers.insert("X-CoreInfra-API-Key", "COREINFRA_API_KEY".into());
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "env_http_headers"],
                headers,
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "requires_openai_auth"],
                true,
            )?;
            config_files::toml::set_value(
                doc,
                &["model_providers", provider, "supports_websockets"],
                true,
            )?;
        }
        RoutingMode::Disabled => {}
    }
    config_files::toml::implicit_table(doc, &["model_providers"])?;
    Ok(())
}

fn remove_inactive_routing_env(
    after: &mut FileSnapshot,
    current_mode: RoutingMode,
    mode: RoutingMode,
) -> Result<()> {
    if mode == RoutingMode::Disabled {
        config_files::env::set_many(after, &[("COREINFRA_API_KEY", None)])?;
    }
    if current_mode == RoutingMode::Sub && mode != RoutingMode::Sub {
        config_files::env::set_many(
            after,
            &[
                ("COREINFRA_PROXY_URL", None),
                ("HTTPS_PROXY", None),
                ("NO_PROXY", None),
            ],
        )?;
    }
    Ok(())
}

fn write_routing_env(after: &mut FileSnapshot, mode: RoutingMode, token: &str) -> Result<()> {
    match mode {
        RoutingMode::Disabled => Ok(()),
        RoutingMode::Hub => {
            config_files::env::set_many(after, &[("COREINFRA_API_KEY", Some(token))])
        }
        RoutingMode::Sub => config_files::env::set_many(
            after,
            &[
                ("COREINFRA_API_KEY", Some(token)),
                (
                    "COREINFRA_PROXY_URL",
                    Some("https://coreinfra:${COREINFRA_API_KEY}@proxy.hub.coreinfra.ai:443"),
                ),
                ("HTTPS_PROXY", Some("${COREINFRA_PROXY_URL}")),
                ("NO_PROXY", Some("localhost,127.0.0.1,::1,hub.coreinfra.ai")),
            ],
        ),
    }
}
