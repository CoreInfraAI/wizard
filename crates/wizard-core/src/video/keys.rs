//! In-memory credentials for one synchronization generation. Never serialize or log keys.
use crate::agents::{claude, claude_desktop, codex};
use anyhow::Result;

#[derive(Default)]
pub struct VideoKeys {
    pub codex: Option<String>,
    pub claude: Option<String>,
    pub claude_desktop: Option<String>,
}

impl VideoKeys {
    /// Blocking file reads. Replace the entire previous snapshot with the returned value.
    #[must_use]
    pub fn load() -> Self {
        fn key(agent: &str, result: Result<Option<String>>) -> Option<String> {
            result.unwrap_or_else(|_| {
                log::warn!(
                    "Could not read {agent} video credential; disabling paid calls for this agent"
                );
                None
            })
        }
        Self {
            codex: key("Codex", codex::active_token()),
            claude: key("Claude Code", claude::configured_token()),
            claude_desktop: key("Claude Desktop", claude_desktop::video_token()),
        }
    }
}
