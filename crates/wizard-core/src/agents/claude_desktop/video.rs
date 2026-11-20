//! Claude Desktop's local 3P profile uses native arrays for both managed extensions.
use crate::video::stdio;
use anyhow::{Context as _, Result, ensure};
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

    pub(super) fn update(&self, profile: &mut Map<String, Value>, enable: bool) -> Result<()> {
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
            |entry| {
                entry["source"] == "github"
                    && entry["repo"] == REPO
                    && entry["expectedName"] == MARKET
                    && entry["credentialKind"] == "anonymous"
                    && entry["installationPreference"] == "auto_install"
                    && entry.get("path").is_none()
            },
            "ref",
        )?;
        let args = json!([stdio::ARG, self.port.to_string(), self.headers]);
        reconcile(
            profile,
            "managedMcpServers",
            enable,
            json!({
                "name":SERVER, "transport":"stdio", "command":self.command, "args":args,
                "toolTimeoutSec":240, "toolPolicy":{"video_models":"allow"}
            }),
            |entry| entry["name"] == SERVER,
            |entry| entry["transport"] == "stdio" && entry["args"] == args,
            "command",
        )
    }
}

// Only refresh the pin or executable when a registration already belongs to Wizard.
// In particular, keep a user's stricter tool policies and other servers in place.
fn reconcile(
    profile: &mut Map<String, Value>,
    key: &str,
    enable: bool,
    desired: Value,
    matches: impl Fn(&Value) -> bool,
    owned: impl Fn(&Value) -> bool,
    refresh: &str,
) -> Result<()> {
    if !enable {
        if let Some(entries) = profile.get_mut(key).and_then(Value::as_array_mut) {
            let previous_len = entries.len();
            entries.retain(|entry| !(matches(entry) && owned(entry)));
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
    let mut existing = entries.iter_mut().filter(|entry| matches(entry));
    if let Some(entry) = existing.next() {
        ensure!(
            owned(entry) && existing.next().is_none(),
            "Конфликт регистрации CoreInfra Video в поле {key} профиля Claude Desktop"
        );
        entry[refresh] = desired[refresh].clone();
    } else {
        entries.push(desired);
    }
    Ok(())
}
