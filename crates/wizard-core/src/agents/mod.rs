//! Agent discovery and configuration shared by GUI and CLI.

use crate::settings::Settings;
use serde::{Deserialize, Serialize};

pub use crate::config_files::backups::{AgentKind as BackupAgent, Backup};

pub mod claude;
pub mod claude_desktop;
pub mod codex;
mod detection;
pub mod opencode;
pub mod pi;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Disabled,
    Sub,
    Hub,
}

/// Agent whose `CoreInfra` Hub token Wizard finds or creates by name.
#[derive(Clone, Copy, Debug)]
pub enum TokenAgent {
    Codex,
    Claude,
    ClaudeDesktop,
    OpenCode,
    Pi,
}

impl TokenAgent {
    /// Name of the Hub token Wizard creates for this agent. Keep in sync with
    /// `createdTokenNames` in `wizard-ui/src/agents/AgentList.tsx`.
    #[must_use]
    pub const fn token_name(self) -> &'static str {
        match self {
            Self::Codex => "agent-codex",
            Self::Claude => "agent-claude-code",
            Self::ClaudeDesktop => "agent-claude-desktop",
            Self::OpenCode => "agent-opencode",
            Self::Pi => "agent-pi",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HubMode {
    Disabled,
    Hub,
}

/// `CoreInfra` token found in an agent's configuration, without `${...}` interpolation.
/// Serialized only for the trusted GUI; Debug never shows the value.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AgentToken(String);

impl AgentToken {
    fn new(token: String) -> Option<Self> {
        (!token.is_empty()).then_some(Self(token))
    }

    fn into_secret(self) -> String {
        self.0
    }
}

impl core::fmt::Debug for AgentToken {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Pi and `OpenCode` keep the Hub key in their auth file as `coreinfra.key`.
fn auth_token(auth: &serde_json::Value) -> Option<AgentToken> {
    let token = auth["coreinfra"]["key"].as_str()?;
    AgentToken::new(token.to_owned())
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentStates {
    codex: AgentDetection<codex::Codex>,
    claude: AgentDetection<claude::Claude>,
    claude_desktop: AgentDetection<claude_desktop::ClaudeDesktop>,
    opencode: AgentDetection<opencode::OpenCode>,
    pi: AgentDetection<pi::Pi>,
}

impl AgentStates {
    /// Replaces token values for output outside the trusted GUI.
    pub fn redact_tokens(&mut self) {
        fn redact(token: &mut Option<AgentToken>) {
            if let Some(token) = token {
                token.0 = "<redacted>".into();
            }
        }

        if let AgentDetection::Found(agent) = &mut self.codex {
            redact(&mut agent.token);
        }
        if let AgentDetection::Found(agent) = &mut self.claude {
            redact(&mut agent.token);
        }
        if let AgentDetection::Found(agent) = &mut self.claude_desktop {
            redact(&mut agent.token);
        }
        if let AgentDetection::Found(agent) = &mut self.opencode {
            redact(&mut agent.token);
        }
        if let AgentDetection::Found(agent) = &mut self.pi {
            redact(&mut agent.token);
        }
    }

    pub fn update_settings_paths(&self, settings: &mut Settings) {
        if let AgentDetection::Found(agent) = &self.codex {
            if let AgentDetection::Found(cli) = &agent.cli {
                settings.last_paths.codex = Some(cli.path.clone());
            }
            if let AgentDetection::Found(desktop) = &agent.desktop {
                settings.last_paths.codex_desktop = Some(desktop.path.clone());
            }
        }
        if let AgentDetection::Found(agent) = &self.claude {
            settings.last_paths.claude = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.claude_desktop {
            settings.last_paths.claude_desktop = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.opencode {
            settings.last_paths.opencode = Some(agent.info.path.clone());
        }
        if let AgentDetection::Found(agent) = &self.pi {
            settings.last_paths.pi = Some(agent.info.path.clone());
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
