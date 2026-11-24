//! Claude Desktop's local 3P profile uses native arrays for both managed extensions.
use crate::media::{install::LEGACY_SERVER, stdio};
use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const MARKET: &str = "coreinfra-wizard";
const REPO: &str = "CoreInfraAI/wizard";
// Desktop requires a published full commit SHA for unattended installation.
// TODO: point at the first published commit containing `plugins/coreinfra-media`.
// Until then Desktop installs the 1.0.10 `coreinfra-video` plugin from this revision.
const REVISION: &str = "b75477dadb850f059279c5a7b46ec5ed291b5e13";
const SERVER: &str = "coreinfra_media";
const CATALOG_TOOLS: &[&str] = &["video_models"];

pub(super) struct Registration {
    pub(super) command: PathBuf,
    pub(super) headers: PathBuf,
    pub(super) port: u16,
}

impl Registration {
    /// Whether `entry` is a Wizard stdio connector (current or 1.0.10), never a foreign server.
    fn owns(&self, entry: &Value) -> bool {
        let Some(args) = entry["args"].as_array() else {
            return false;
        };
        if args.len() != 3 {
            return false;
        }
        let flag = args[0]
            .as_str()
            .is_some_and(|arg| arg == stdio::ARG || arg == stdio::LEGACY_ARG);
        let port = args[1].as_str() == Some(self.port.to_string().as_str());
        let path = args[2].as_str().map(std::path::Path::new);
        let in_wizard_dir = self
            .headers
            .parent()
            .is_some_and(|dir| path.and_then(std::path::Path::parent) == Some(dir));
        let headers = path == Some(self.headers.as_path())
            || crate::media::install::legacy_headers(&self.headers)
                .is_some_and(|legacy| path == Some(legacy.as_path()))
            || (in_wizard_dir
                && path
                    .and_then(|p| p.file_name()?.to_str())
                    .is_some_and(|name| name.ends_with("-headers.json")));
        flag && port && headers
    }

    fn matches_owned(&self, name: &str, entry: &Value) -> bool {
        (name == SERVER || name == LEGACY_SERVER) && self.owns(entry)
    }

    pub(super) fn for_wizard() -> Result<Self> {
        Self::with_headers(crate::media::install::headers_path(true)?)
    }

    /// Tests keep the grant file out of the user's Wizard configuration.
    pub(super) fn with_headers(headers: PathBuf) -> Result<Self> {
        let command = std::env::current_exe()?;
        // AppImage remounts its contents at a new temporary path on every launch.
        #[cfg(target_os = "linux")]
        let command = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|path| {
                path.is_absolute()
                    && std::env::var_os("APPDIR").is_some_and(|dir| command.starts_with(dir))
            })
            .unwrap_or(command);
        Ok(Self {
            command,
            headers,
            port: crate::media::ADDRESS.port(),
        })
    }

    pub(super) fn update_3p_profile(
        &self,
        profile: &mut Map<String, Value>,
        enable: bool,
    ) -> Result<()> {
        // Check for a foreign coreinfra_media server before mutating any profile key.
        let existing_servers = profile.get("managedMcpServers").and_then(Value::as_array);
        if enable
            && existing_servers.is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["name"] == SERVER && !self.owns(entry))
            })
        {
            anyhow::bail!(
                "В профиле Claude Desktop уже зарегистрирован другой MCP-сервер coreinfra_media"
            );
        }
        // Preserve any user toolPolicy overrides/restrictions on an existing owned entry.
        let mut policy = existing_servers
            .and_then(|entries| {
                entries.iter().find(|entry| {
                    entry["name"]
                        .as_str()
                        .is_some_and(|name| self.matches_owned(name, entry))
                })
            })
            .and_then(|entry| entry["toolPolicy"].as_object())
            .cloned()
            .unwrap_or_default();
        for &tool in CATALOG_TOOLS {
            policy.entry(tool).or_insert_with(|| json!("allow"));
        }

        reconcile(
            profile,
            "allowedPluginMarketplaces",
            enable,
            json!({
                "source":"github", "repo":REPO, "expectedName":MARKET, "ref":REVISION,
                "credentialKind":"anonymous", "installationPreference":"auto_install"
            }),
            |entry| {
                entry["expectedName"] == MARKET
                    || entry["repo"]
                        .as_str()
                        .is_some_and(|repo| repo.eq_ignore_ascii_case(REPO))
            },
        )?;
        reconcile(
            profile,
            "managedMcpServers",
            enable,
            json!({
                "name":SERVER, "transport":"stdio", "command":self.command, "args":self.args(),
                "toolTimeoutSec":240, "toolPolicy":policy
            }),
            |entry| {
                entry["name"]
                    .as_str()
                    .is_some_and(|name| self.matches_owned(name, entry))
            },
        )
    }

    fn args(&self) -> Value {
        json!([stdio::ARG, self.port.to_string(), self.headers])
    }

    /// Claude Desktop reads local servers from `mcpServers` in its 1p configuration.
    pub(super) fn update_1p_config(
        &self,
        config: &mut Map<String, Value>,
        enable: bool,
    ) -> Result<()> {
        if enable
            && config
                .get("mcpServers")
                .and_then(Value::as_object)
                .and_then(|servers| servers.get(SERVER))
                .is_some_and(|entry| !self.owns(entry))
        {
            anyhow::bail!(
                "В настройках Claude Desktop уже зарегистрирован другой MCP-сервер coreinfra_media"
            );
        }
        if let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) {
            let before = servers.len();
            servers.retain(|name, entry| !self.matches_owned(name, entry));
            if before != servers.len() && servers.is_empty() {
                config.shift_remove("mcpServers");
            }
        }
        if !enable {
            return Ok(());
        }
        let servers = config
            .entry("mcpServers")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("В настройках Claude Desktop поле mcpServers должно быть объектом")?;
        servers.insert(
            SERVER.into(),
            json!({"command": self.command, "args": self.args()}),
        );
        Ok(())
    }
}

// Replace all versions of the registration at its first position in the profile.
fn reconcile(
    profile: &mut Map<String, Value>,
    key: &str,
    enable: bool,
    desired: Value,
    matches: impl Fn(&Value) -> bool,
) -> Result<()> {
    if !enable {
        if let Some(entries) = profile.get_mut(key).and_then(Value::as_array_mut) {
            let previous_len = entries.len();
            entries.retain(|entry| !matches(entry));
            if previous_len != 0 && entries.is_empty() {
                profile.shift_remove(key);
            }
        }
        return Ok(());
    }

    let entries = profile
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .with_context(|| format!("В профиле Claude Desktop поле {key} должно быть массивом"))?;
    let index = entries.iter().position(&matches).unwrap_or(entries.len());
    entries.retain(|entry| !matches(entry));
    entries.insert(index, desired);
    Ok(())
}
