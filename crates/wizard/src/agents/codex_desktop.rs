#[cfg(target_os = "macos")]
use std::env;
use std::path::PathBuf;

use serde::Serialize;

use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CodexDesktop {
    pub path: PathBuf,
    pub version: Option<String>,
}

pub(super) fn detect() -> AgentDetection<CodexDesktop> {
    detect_desktop()
}

#[cfg(target_os = "macos")]
fn detect_desktop() -> AgentDetection<CodexDesktop> {
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
            Ok(version) => AgentDetection::Found(CodexDesktop { path, version }),
            Err(error) => AgentDetection::failed(&path, &error),
        };
    }
    AgentDetection::NotFound
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn detect_desktop() -> AgentDetection<CodexDesktop> {
    AgentDetection::Error("not supported".to_owned())
}
