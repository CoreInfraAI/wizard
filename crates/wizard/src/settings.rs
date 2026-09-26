//! Backend-only settings storage. The API key is stored as plaintext, never logged.

use std::{fs, io::Write as _, path::PathBuf, sync::Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;

// No Debug: settings contain a secret and must not be logged.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub coreinfra_api_key: String,
}

pub(crate) async fn initialize_state(app: &tauri::AppHandle) -> Result<()> {
    let settings = tauri::async_runtime::spawn_blocking(load_from_file)
        .await
        .context("settings initialization task failed")??;
    if !app.manage(Mutex::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
}

/// TODO: update state when file changes
pub async fn get_state(app: &tauri::AppHandle) -> Result<Settings> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<Mutex<Settings>>()
            .context("settings not initialized")?;
        let current = state
            .lock()
            .map_err(|_| anyhow!("settings lock poisoned"))?;
        Ok(current.clone())
    })
    .await
    .context("settings read task failed")?
}

/// on save error still updates state
pub async fn update_state(
    app: &tauri::AppHandle,
    edit: impl FnOnce(&mut Settings) + Send + 'static,
) -> Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<Mutex<Settings>>()
            .context("settings not initialized")?;
        let mut current = state
            .lock()
            .map_err(|_| anyhow!("settings lock poisoned"))?;
        edit(&mut current);
        save_to_file(&current)?;
        Ok(())
    })
    .await
    .context("settings save task failed")?
}

fn settings_path() -> Result<PathBuf> {
    // match `identifier` in crates/wizard/tauri.conf.json.
    const IDENTIFIER: &str = "ai.coreinfra.wizard";
    dirs::config_dir()
        .map(|dir| dir.join(IDENTIFIER).join("config.toml"))
        .context("failed to resolve Wizard settings directory")
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
    let mut settings = load_from_file()?;
    edit(&mut settings);
    save_to_file(&settings)
}
