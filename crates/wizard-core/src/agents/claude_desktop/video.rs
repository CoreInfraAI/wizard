//! Claude Desktop's local 3P profile uses native arrays for both managed extensions.
use crate::video::stdio;
use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

const MARKET: &str = "coreinfra-wizard";
const REPO: &str = "CoreInfraAI/wizard";
// Desktop requires a published full commit SHA for unattended installation.
const REVISION: &str = "b75477dadb850f059279c5a7b46ec5ed291b5e13";
const SERVER: &str = "coreinfra_video";

pub(super) struct Registration {
    pub(super) command: PathBuf,
    pub(super) headers: PathBuf,
    pub(super) port: u16,
}

impl Registration {
    pub(super) fn for_wizard() -> Result<Self> {
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
            headers: crate::video::install::headers_path(true)?,
            port: crate::video::ADDRESS.port(),
        })
    }

    pub(super) fn update_3p_profile(
        &self,
        profile: &mut Map<String, Value>,
        enable: bool,
    ) -> Result<()> {
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
                "toolTimeoutSec":240, "toolPolicy":{"video_models":"allow"}
            }),
            |entry| entry["name"] == SERVER,
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
        if !enable {
            if let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut)
                && servers.shift_remove(SERVER).is_some()
                && servers.is_empty()
            {
                config.shift_remove("mcpServers");
            }
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
