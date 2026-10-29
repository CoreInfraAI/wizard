//! Backend-only settings storage. Secrets (Hub session, proxy tokens) are stored as plaintext,
//! never logged.

pub mod paths;

use anyhow::{Context as _, Result, anyhow, bail};
use paths::settings_path;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write as _, path::PathBuf};

// No Debug: settings contain a secret and must not be logged.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// Local proxy demand of Claude Desktop with its token; `None` means disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_proxy_enabled: Option<String>,
    /// Local proxy demand of `ChatGPT` with its token; `None` means disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_proxy_enabled: Option<String>,
    /// Account session, independent of agent tokens. Never expose through IPC.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hub_session: String,
    /// Selected Hub for listing and creating agent tokens; empty uses the personal hub.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_hub_id: String,
    /// Application lifecycle preferences, independent of proxy demand.
    pub application: ApplicationSettings,
    /// Kept last: it serializes as a TOML table, written after plain keys.
    pub last_paths: LastPaths,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ApplicationSettings {
    pub tray_enabled: bool,
    pub autostart_enabled: bool,
}

impl Default for ApplicationSettings {
    fn default() -> Self {
        Self {
            tray_enabled: true,
            autostart_enabled: true,
        }
    }
}

/// Last found agent paths: first search candidates, not cached detection results.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LastPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_desktop: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_desktop: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi: Option<PathBuf>,
}

impl Settings {
    #[must_use]
    pub fn local_proxy_enabled(&self) -> bool {
        self.claude_proxy_enabled.is_some() || self.codex_proxy_enabled.is_some()
    }
}

pub fn load_from_file() -> Result<Settings> {
    let text = match fs::read_to_string(settings_path()?) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(error).context("failed to read Wizard settings"),
    };
    // Do not retain the parser error: it may contain the API key.
    toml_edit::de::from_str(&text).map_err(|_| anyhow!("failed to deserialize Wizard settings"))
}

pub fn save_to_file(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    let serialized = toml_edit::ser::to_string_pretty(settings)
        .map_err(|_| anyhow!("failed to serialize Wizard settings"))?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("Wizard settings file is a symlink; refusing to replace it");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect Wizard settings file"),
    }
    let parent = path.parent().context("settings path has no parent")?;
    fs::create_dir_all(parent).context("failed to create Wizard settings directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create temporary Wizard settings file")?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to restrict Wizard settings permissions")?;
    }
    file.write_all(serialized.as_bytes())
        .context("failed to write Wizard settings")?;
    file.as_file()
        .sync_all()
        .context("failed to sync Wizard settings")?;
    file.persist(path)
        .context("failed to save Wizard settings")?;
    Ok(())
}

pub fn update_file(edit: impl FnOnce(&mut Settings)) -> Result<()> {
    let current = load_from_file()?;
    let mut settings = current.clone();
    edit(&mut settings);
    if settings == current {
        return Ok(());
    }
    save_to_file(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_defaults_and_partial_tables() {
        for text in ["", "[last_paths]\n", "[application]\n"] {
            let settings: Settings = toml_edit::de::from_str(text).expect("valid settings");
            assert!(settings.application.tray_enabled);
            assert!(settings.application.autostart_enabled);
        }
        let settings: Settings = toml_edit::de::from_str("[application]\ntray_enabled = false\n")
            .expect("partial application settings");
        assert!(!settings.application.tray_enabled);
        assert!(settings.application.autostart_enabled);
        assert!(Settings::default().application.tray_enabled);
        assert!(Settings::default().application.autostart_enabled);
    }

    #[test]
    fn application_preferences_round_trip_independently_of_proxy_demand() {
        for tray_enabled in [false, true] {
            for autostart_enabled in [false, true] {
                let settings = Settings {
                    application: ApplicationSettings {
                        tray_enabled,
                        autostart_enabled,
                    },
                    claude_proxy_enabled: Some("fixture".into()),
                    ..Settings::default()
                };
                let text = toml_edit::ser::to_string_pretty(&settings).expect("serialize settings");
                let restored: Settings = toml_edit::de::from_str(&text).expect("read settings");
                assert_eq!(restored.application.tray_enabled, tray_enabled);
                assert_eq!(restored.application.autostart_enabled, autostart_enabled);
                assert!(restored.local_proxy_enabled());
            }
        }
    }

    #[test]
    fn existing_settings_default_to_personal_hub() {
        let settings: Settings = toml_edit::de::from_str("hub_session = 'session'\n[last_paths]\n")
            .expect("settings without a selected hub should deserialize");
        assert!(settings.active_hub_id.is_empty());
    }

    #[test]
    fn selected_hub_round_trips_with_discovered_paths() {
        let settings = Settings {
            active_hub_id: "team".into(),
            last_paths: LastPaths {
                codex: Some(PathBuf::from("/usr/local/bin/codex")),
                ..LastPaths::default()
            },
            ..Settings::default()
        };
        let text = toml_edit::ser::to_string_pretty(&settings)
            .expect("settings with a selected hub and paths should serialize");
        let restored: Settings =
            toml_edit::de::from_str(&text).expect("serialized settings should deserialize");
        assert_eq!(restored.active_hub_id, "team");
        assert_eq!(restored.last_paths.codex, settings.last_paths.codex);
    }
}
