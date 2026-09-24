//! Backend-only settings storage. The API key is stored as plaintext, never logged.

use std::{fs, io::Write as _, path::PathBuf, sync::Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;

// No Debug: settings contain a secret and must not be logged.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coreinfra_api_key: Option<String>,
}

pub(crate) async fn initialize(app: &tauri::AppHandle) -> Result<()> {
    let handle = app.clone();
    let settings = tauri::async_runtime::spawn_blocking(move || load(&handle))
        .await
        .context("settings initialization task failed")??;
    if !app.manage(Mutex::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
}

pub async fn get(app: &tauri::AppHandle) -> Result<Settings> {
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

pub async fn set(app: &tauri::AppHandle, settings: Settings) -> Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<Mutex<Settings>>()
            .context("settings not initialized")?;
        let mut current = state
            .lock()
            .map_err(|_| anyhow!("settings lock poisoned"))?;
        save(&app, &settings)?;
        *current = settings;
        Ok(())
    })
    .await
    .context("settings save task failed")?
}

fn path(app: &tauri::AppHandle) -> Result<PathBuf> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("config.toml"))
        .context("failed to resolve Wizard settings directory")
}

fn load(app: &tauri::AppHandle) -> Result<Settings> {
    let text = match fs::read_to_string(path(app)?) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(error).context("failed to read Wizard settings"),
    };
    // Do not retain the parser error: it may contain the API key.
    toml_edit::de::from_str(&text).map_err(|_| anyhow!("failed to deserialize Wizard settings"))
}

fn save(app: &tauri::AppHandle, settings: &Settings) -> Result<()> {
    let path = path(app)?;
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
    file.persist(&path)
        .context("failed to save Wizard settings")?;
    Ok(())
}
