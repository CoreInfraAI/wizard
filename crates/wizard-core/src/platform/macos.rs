use anyhow::{Context as _, Result};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSProcessInfo, NSString};
use serde::Deserialize;
use std::path::Path;

pub(crate) fn os_version() -> String {
    let version = NSProcessInfo::processInfo().operatingSystemVersion();

    format!(
        "{}.{}.{}",
        version.majorVersion, version.minorVersion, version.patchVersion,
    )
}

/// Checks all running instances by bundle identifier without launching or closing them.
pub(crate) fn app_is_running(bundle_id: &str) -> bool {
    let bundle_id = NSString::from_str(bundle_id);
    let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id);

    applications.count() != 0
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_os_version_is_numeric() {
        let version = os_version();
        let parts: Vec<_> = version.split('.').collect();

        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.parse::<u64>().is_ok()));
        assert_ne!(parts[0], "0");
    }

    #[test]
    fn nonexistent_application_is_not_running() {
        let bundle_id = format!("ai.coreinfra.wizard.test.{}", uuid::Uuid::new_v4());

        assert!(!app_is_running(&bundle_id));
    }
}
