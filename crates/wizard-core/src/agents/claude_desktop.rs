#[cfg(target_os = "macos")]
use std::env;
use std::path::PathBuf;

use serde::Serialize;

use super::AgentDetection;
#[cfg(target_os = "macos")]
use crate::platform;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ClaudeDesktop {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[cfg(target_os = "macos")]
pub(super) fn detect() -> AgentDetection<ClaudeDesktop> {
    let mut candidates = vec![PathBuf::from("/Applications/Claude.app")];
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Applications/Claude.app"));
    }
    for path in candidates {
        match path.try_exists() {
            Ok(false) => continue,
            Err(error) => return AgentDetection::failed(&path, &error),
            Ok(true) => {}
        }
        return match platform::macos::read_app_version(&path) {
            Ok(version) => AgentDetection::Found(ClaudeDesktop { path, version }),
            Err(error) => AgentDetection::failed(&path, &error),
        };
    }
    AgentDetection::NotFound
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) fn detect() -> AgentDetection<ClaudeDesktop> {
    AgentDetection::Error("not supported".to_owned())
}
