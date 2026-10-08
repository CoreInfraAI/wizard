//! Read-only viewer for the current GUI log file.

use std::{
    fs::File,
    io::{Read as _, Seek as _, SeekFrom},
    path::Path,
};

use anyhow::{Context as _, Result};
use tauri::Manager as _;

const WINDOW_LABEL: &str = "logs";
const MAX_LOG_BYTES: u64 = 256 * 1024;

#[tauri::command]
pub(crate) async fn open_logs(app: tauri::AppHandle) -> Result<(), String> {
    let result = (|| -> Result<()> {
        let window = if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            window
        } else {
            tauri::WebviewWindowBuilder::new(
                &app,
                WINDOW_LABEL,
                tauri::WebviewUrl::App("index.html#logs".into()),
            )
            .title(
                app.get_webview_window(crate::MAIN_WINDOW_NAME)
                    .context("main window is unavailable")?
                    .title()?,
            )
            .inner_size(1000.0, 650.0)
            .min_inner_size(480.0, 320.0)
            .build()
            .context("failed to create log window")?
        };
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
        Ok(())
    })();
    result.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub(crate) async fn read_logs(app: tauri::AppHandle) -> Result<String, String> {
    let path = app
        .path()
        .app_log_dir()
        .map_err(|error| error.to_string())?
        .join("Wizard.log");
    tauri::async_runtime::spawn_blocking(move || read_tail(&path))
        .await
        .map_err(|error| format!("log reader task failed: {error}"))?
        .map_err(|error| format!("{error:#}"))
}

fn read_tail(path: &Path) -> Result<String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error).context("failed to open log file"),
    };
    let offset = file.metadata()?.len().saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_BYTES)
        .read_to_end(&mut bytes)
        .context("failed to read log file")?;
    // Omit the partial first line when reading only the tail.
    let start = if offset == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}
