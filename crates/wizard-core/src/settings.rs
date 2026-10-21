//! Backend-only settings storage. Secrets (Hub session, proxy tokens) are stored as plaintext,
//! never logged.

use anyhow::{Context as _, Result, anyhow, bail};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_desktop_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_desktop_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi_path_last: Option<PathBuf>,
    /// Account shown in the Hub tile, captured from `/hub/api/profile` at login.
    /// Kept last: it serializes as a TOML table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub_profile: Option<HubProfile>,
}

/// Non-secret Hub account details. Deserializing a profile response keeps only these fields.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubProfile {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_email: Option<String>,
    /// Hub whose agent tokens Wizard lists. Empty in profiles saved before it was stored.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub personal_hub_id: String,
}

impl Settings {
    #[must_use]
    pub fn local_proxy_enabled(&self) -> bool {
        self.claude_proxy_enabled.is_some() || self.codex_proxy_enabled.is_some()
    }

    /// Token for the shared local proxy: Claude Desktop's, else `ChatGPT`'s.
    // TODO: Route each desktop app with its own token.
    #[must_use]
    pub fn local_proxy_token(&self) -> Option<&str> {
        self.claude_proxy_enabled
            .as_deref()
            .or(self.codex_proxy_enabled.as_deref())
    }
}

fn settings_path() -> Result<PathBuf> {
    Ok(crate::config_dir()?.join("config.toml"))
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
