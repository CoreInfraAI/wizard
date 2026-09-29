//! Agent discovery and configuration shared by GUI and CLI.

use serde::Serialize;

use crate::settings;

pub mod chatgpt;
pub mod claude;
pub mod claude_desktop;
pub mod codex;
mod detection;
pub mod opencode;
pub mod pi;

#[derive(Debug, Serialize)]
pub struct AgentState {
    agents: AgentStates,
    coreinfra_token_set: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStates {
    pub codex: AgentDetection<codex::Codex>,
    pub chatgpt: AgentDetection<chatgpt::ChatGpt>,
    pub claude: AgentDetection<claude::Claude>,
    pub claude_desktop: AgentDetection<claude_desktop::ClaudeDesktop>,
    pub opencode: AgentDetection<opencode::OpenCode>,
    pub pi: AgentDetection<pi::Pi>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
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
pub async fn collect_agent_state(settings: &settings::Settings) -> AgentState {
    let agents = detect().await;
    let coreinfra_token_set = !settings.coreinfra_api_key.is_empty();

    AgentState {
        agents,
        coreinfra_token_set,
    }
}

async fn detect() -> AgentStates {
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

    // Start all detectors before awaiting their results so they can run concurrently.
    let codex = tokio::task::spawn_blocking(codex::detect);
    let chatgpt = tokio::task::spawn_blocking(chatgpt::detect);
    let claude = tokio::task::spawn_blocking(claude::detect);
    let claude_desktop = tokio::task::spawn_blocking(claude_desktop::detect);
    let opencode = tokio::task::spawn_blocking(opencode::detect);
    let pi = tokio::task::spawn_blocking(pi::detect);

    AgentStates {
        codex: collect("Codex", codex).await,
        chatgpt: collect("ChatGPT", chatgpt).await,
        claude: collect("Claude Code", claude).await,
        claude_desktop: collect("Claude Desktop", claude_desktop).await,
        opencode: collect("OpenCode", opencode).await,
        pi: collect("pi", pi).await,
    }
}
