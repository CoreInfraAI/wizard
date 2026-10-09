//! Backend-only settings storage. The API key is stored as plaintext, never logged.

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write as _, path::PathBuf};

// No Debug: settings contain a secret and must not be logged.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    #[serde(rename = "coreinfra_api_key", skip_serializing_if = "String::is_empty")]
    pub coreinfra_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chatgpt_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_desktop_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode_path_last: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi_path_last: Option<PathBuf>,
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
