#[cfg(target_os = "macos")]
use std::{env, path::PathBuf};

use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform;

pub(super) fn detect() -> AgentDetection {
    detect_desktop()
}

#[cfg(target_os = "macos")]
fn detect_desktop() -> AgentDetection {
    let mut candidates = vec![PathBuf::from("/Applications/ChatGPT.app")];
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/ChatGPT.app"));
    }
    for path in candidates {
        match path.try_exists() {
            Ok(false) => continue,
            Err(error) => return AgentDetection::failed(&path, &error),
            Ok(true) => {}
        }
        return match platform::macos::read_app_version(&path) {
            Ok(version) => AgentDetection::found(path, version),
            Err(error) => AgentDetection::failed(&path, &error),
        };
    }
    AgentDetection::NotFound
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn detect_desktop() -> AgentDetection {
    AgentDetection::Error("not supported".to_owned())
}
