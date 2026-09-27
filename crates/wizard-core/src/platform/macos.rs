use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Deserialize;

#[derive(Deserialize)]
struct AppInfo {
    #[serde(rename = "CFBundleShortVersionString")]
    version: Option<String>,
}

/// Reads an app bundle's version without launching the application.
/// `plist` handles both XML and binary Info.plist files.
pub(crate) fn read_app_version(app: &Path) -> Result<Option<String>> {
    let path = app.join("Contents/Info.plist");
    log::debug!("reading application metadata: {}", path.display());
    let info: AppInfo =
        plist::from_file(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(info.version.filter(|version| !version.trim().is_empty()))
}
