use serde::Serialize;

use super::{
    AgentDetection,
    detection::{self, Agent, AgentInfo},
};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Pi {
    #[serde(flatten)]
    pub info: AgentInfo,
}

pub(super) fn detect() -> AgentDetection<Pi> {
    let info = match detection::detect(Agent::Pi) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    AgentDetection::Found(Pi { info })
}
