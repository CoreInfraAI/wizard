//! Agent discovery and configuration shared by GUI and CLI.

use crate::settings::Settings;
use serde::Serialize;

pub use crate::config_files::backups::{AgentKind as BackupAgent, Backup};

pub mod claude;
pub mod claude_desktop;
pub mod codex;
mod detection;
pub mod opencode;
pub mod pi;

#[derive(Clone, Debug, Serialize)]
pub struct AgentStates {
    codex: AgentDetection<codex::Codex>,
    claude: AgentDetection<claude::Claude>,
    claude_desktop: AgentDetection<claude_desktop::ClaudeDesktop>,
    opencode: AgentDetection<opencode::OpenCode>,
    pi: AgentDetection<pi::Pi>,
}

impl AgentStates {
    pub fn update_settings_paths(&self, settings: &mut Settings) {
        if let AgentDetection::Found(agent) = &self.codex {
            if let AgentDetection::Found(cli) = &agent.cli {
                settings.codex_path_last = Some(cli.path.clone());
            }
            if let AgentDetection::Found(desktop) = &agent.desktop {
                settings.codex_desktop_path_last = Some(desktop.path.clone());
            }
        }
        if let AgentDetection::Found(agent) = &self.claude {
            settings.claude_path_last = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.claude_desktop {
            settings.claude_desktop_path_last = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.opencode {
            settings.opencode_path_last = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.pi {
            settings.pi_path_last = Some(agent.info.path.clone());
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum AgentDetection<T> {
    Found(T),
    NotFound,
    Error(String),
}

impl<T> AgentDetection<T> {
    fn failed(path: &std::path::Path, error: &impl core::fmt::Display) -> Self {
        Self::Error(format!("{}: {error:#}", path.display()))
    }
}

/// Collects state for GUI or CLI. Must be called within a Tokio runtime with I/O and time enabled.
pub async fn collect_agent_state(settings: &Settings) -> AgentStates {
    async fn collect<T: core::fmt::Debug>(
        name: &str,
        task: tokio::task::JoinHandle<AgentDetection<T>>,
    ) -> AgentDetection<T> {
        let result = task
            .await
            .unwrap_or_else(|error| AgentDetection::Error(error.to_string()));
        match &result {
            AgentDetection::Found(agent) => log::debug!("found {name}: {agent:?}"),
            AgentDetection::NotFound => log::debug!("{name} not found"),
            AgentDetection::Error(error) => log::error!("{name} detection failed: {error}"),
        }
        result
    }

    fn spawn_detection<T: Send + 'static>(
        settings: &Settings,
        detect: fn(&Settings) -> AgentDetection<T>,
    ) -> tokio::task::JoinHandle<AgentDetection<T>> {
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || detect(&settings))
    }

    // Start all detectors before awaiting their results so they can run concurrently.
    let codex = spawn_detection(settings, codex::detect);
    let claude = spawn_detection(settings, claude::detect);
    let claude_desktop = spawn_detection(settings, claude_desktop::detect);
    let opencode = spawn_detection(settings, opencode::detect);
    let pi = spawn_detection(settings, pi::detect);

    AgentStates {
        codex: collect("Codex", codex).await,
        claude: collect("Claude Code", claude).await,
        claude_desktop: collect("Claude Desktop", claude_desktop).await,
        opencode: collect("OpenCode", opencode).await,
        pi: collect("pi", pi).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentDetection, AgentStates, codex, detection::AgentInfo};
    use crate::settings::Settings;
    use std::path::PathBuf;

    fn codex_state(
        cli: AgentDetection<AgentInfo>,
        desktop: AgentDetection<AgentInfo>,
    ) -> AgentStates {
        AgentStates {
            codex: AgentDetection::Found(codex::Codex {
                cli,
                desktop,
                proxy_mode: codex::ProxyMode::ProxyHub,
            }),
            claude: AgentDetection::NotFound,
            claude_desktop: AgentDetection::NotFound,
            opencode: AgentDetection::NotFound,
            pi: AgentDetection::NotFound,
        }
    }

    fn installation(path: &str) -> AgentDetection<AgentInfo> {
        AgentDetection::Found(AgentInfo {
            path: PathBuf::from(path),
            version: None,
        })
    }

    #[test]
    fn shared_codex_state_serializes_one_plain_proxy_mode() {
        let state = codex_state(AgentDetection::NotFound, installation("ChatGPT.app"));
        let value = serde_json::to_value(state).expect("serialize state");
        assert!(value.get("codex_desktop").is_none());
        assert_eq!(value["codex"]["status"], "found");
        let codex = &value["codex"]["data"];
        assert_eq!(codex["proxy_mode"], "proxy_hub");
        assert_eq!(codex["cli"]["status"], "not_found");
        assert_eq!(codex["desktop"]["status"], "found");
    }

    #[test]
    fn remembers_both_installation_paths() {
        let state = codex_state(installation("codex"), installation("ChatGPT.app"));
        let mut settings = Settings::default();
        state.update_settings_paths(&mut settings);
        assert_eq!(settings.codex_path_last, Some(PathBuf::from("codex")));
        assert_eq!(
            settings.codex_desktop_path_last,
            Some(PathBuf::from("ChatGPT.app"))
        );
    }

    #[test]
    fn desktop_only_preserves_last_known_cli_path() {
        let state = codex_state(AgentDetection::NotFound, installation("ChatGPT.app"));
        let mut settings = Settings {
            codex_path_last: Some(PathBuf::from("previous-codex")),
            ..Settings::default()
        };
        state.update_settings_paths(&mut settings);
        assert_eq!(
            settings.codex_path_last,
            Some(PathBuf::from("previous-codex"))
        );
        assert_eq!(
            settings.codex_desktop_path_last,
            Some(PathBuf::from("ChatGPT.app"))
        );
    }
}
