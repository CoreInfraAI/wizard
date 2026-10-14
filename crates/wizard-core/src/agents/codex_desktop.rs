use super::{
    AgentDetection,
    detection::{self, Agent, AgentInfo},
};
use crate::settings::Settings;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CodexDesktop {
    #[serde(flatten)]
    pub info: AgentInfo,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<CodexDesktop> {
    let info = match detection::detect(
        Agent::CodexDesktop,
        settings.codex_desktop_path_last.as_deref(),
    ) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    AgentDetection::Found(CodexDesktop { info })
}
