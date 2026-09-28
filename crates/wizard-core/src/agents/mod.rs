//! Agent discovery and configuration shared by GUI and CLI.

use serde::Serialize;

use crate::settings;

pub mod chatgpt;
pub mod codex;

#[derive(Debug, Serialize)]
pub struct AgentState {
    agents: AgentStates,
    coreinfra_token_set: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentStates {
    pub codex: AgentDetection<codex::Codex>,
    pub chatgpt: AgentDetection<chatgpt::ChatGpt>,
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
    // Start all detectors before awaiting their results so they can run concurrently.
    let codex = tokio::task::spawn_blocking(codex::detect);
    let chatgpt = tokio::task::spawn_blocking(chatgpt::detect);

    AgentStates {
        codex: codex
            .await
            .unwrap_or_else(|error| AgentDetection::Error(error.to_string())),
        chatgpt: chatgpt
            .await
            .unwrap_or_else(|error| AgentDetection::Error(error.to_string())),
    }
}
