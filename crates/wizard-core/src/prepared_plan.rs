//! Serializable prepare / confirm / apply contract shared by CLI and GUI.
//! Only execute plans from trusted preparation: serialization is not authorization.
//! Snapshots contain credentials and must never be logged or printed as a plan preview.
//! External commands have no rollback; `@latest` does not pin a package version.

use crate::{
    agents::{claude, claude_desktop, codex, opencode, pi},
    config_files::{
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::MutexGuard};

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PreparedAgent {
    Codex,
    Claude,
    ClaudeDesktop,
    Pi,
    OpenCode,
}

impl PreparedAgent {
    pub(crate) fn lock(self) -> Result<MutexGuard<'static, ()>> {
        let mutex = match self {
            Self::Codex => &codex::CONFIG_LOCK,
            Self::Claude => &claude::CONFIG_LOCK,
            Self::ClaudeDesktop => &claude_desktop::CONFIG_LOCK,
            Self::Pi => &pi::CONFIG_LOCK,
            Self::OpenCode => &opencode::CONFIG_LOCK,
        };
        mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("agent configuration lock poisoned"))
    }
}

// No Debug: the original and prepared contents may contain credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPlan {
    agent: PreparedAgent,
    pub(crate) steps: Vec<PreparedStep>,
}

impl PreparedPlan {
    pub(crate) fn new(agent: PreparedAgent, steps: Vec<PreparedStep>) -> Self {
        Self { agent, steps }
    }

    #[must_use]
    pub fn steps(&self) -> &[PreparedStep] {
        &self.steps
    }

    /// Safe preview: omit unchanged files without removing steps from the plan.
    /// Never include file contents or credentials.
    #[must_use]
    pub fn info(&self) -> Vec<PreparedStepInfo> {
        let mut info = Vec::new();

        for step in &self.steps {
            let item = match step {
                PreparedStep::ChangeFile { change } | PreparedStep::PiApplyAuth { change, .. }
                    if !change.is_changed() => {
                    continue
                },
                PreparedStep::ChangeFile { change } => {
                    let action = match (&change.after, change.before()) {
                        (FileSnapshot::Missing, _) => "Удалить файл",
                        (_, FileSnapshot::Missing) => "Создать файл",
                        _ => "Изменить файл",
                    };
                    PreparedStepInfo {
                        description: format!("{action} {}", change.path().display()),
                    }
                },
                PreparedStep::PiApplyAuth { change, .. } => PreparedStepInfo {
                    description: format!("Записать настройки доступа Pi в {}.", change.path().display()),
                },
                PreparedStep::PiInstallPlugin { .. } => PreparedStepInfo {
                    description: "Установить последнюю версию плагина CoreInfra для Pi.".to_owned(),
                },
                PreparedStep::PiRemovePlugin { .. } => PreparedStepInfo {
                    description: "Удалить плагин CoreInfra из Pi.".to_owned(),
                },
                PreparedStep::OpenCodeInstallPlugin { .. } => PreparedStepInfo {
                    description: "Установить последнюю версию плагина CoreInfra для OpenCode.".to_owned(),
                },
                PreparedStep::OpenCodeVerifyPlugin { .. } => PreparedStepInfo {
                    description: "Убедиться, что плагин CoreInfra добавлен в конфигурацию OpenCode.".to_owned(),
                },
                PreparedStep::SetCodexProxyEnabled { token } => PreparedStepInfo {
                    description: if token.is_some() {
                        "Включить локальный прокси ChatGPT в Wizard."
                    } else {
                        "Выключить локальный прокси ChatGPT в Wizard."
                    }.to_owned(),
                },
                PreparedStep::SetClaudeProxyEnabled { token } => PreparedStepInfo {
                    description: if token.is_some() {
                        "Включить локальный прокси Claude Desktop в Wizard."
                    } else {
                        "Выключить локальный прокси Claude Desktop в Wizard."
                    }.to_owned(),
                },
                PreparedStep::CodexRestartDaemon { .. } => PreparedStepInfo {
                    description: "Перезапустить фоновую службу Codex, если она запущена. Это может прервать текущую работу.".to_owned(),
                },
            };
            info.push(item);
        }
        info
    }

    /// Call from a Tokio blocking worker. Adjacent file changes are applied as a group.
    /// Codex/Claude backups cover the entire plan; there is no rollback on failure.
    pub fn apply(self) -> Result<()> {
        anyhow::ensure!(
            !self.steps.iter().any(|step| matches!(
                step,
                PreparedStep::SetClaudeProxyEnabled { .. }
                    | PreparedStep::SetCodexProxyEnabled { .. }
            )),
            "Desktop proxy plans can only be applied in the GUI"
        );
        self.apply_with_proxies(
            |_| anyhow::bail!("Claude Desktop proxy requires the GUI"),
            |_| anyhow::bail!("ChatGPT proxy requires the GUI"),
        )
    }

    /// Apply with the GUI-owned settings/runtime operation, under the agent lock.
    /// The callback must not acquire the agent lock or trigger agent discovery.
    /// Callbacks receive the app's proxy token, or `None` to disable its proxy demand.
    pub fn apply_with_proxies(
        self,
        mut set_claude_enabled: impl FnMut(Option<String>) -> Result<()>,
        mut set_codex_enabled: impl FnMut(Option<String>) -> Result<()>,
    ) -> Result<()> {
        let _guard = self.agent.lock()?;

        let backup_agent = match self.agent {
            PreparedAgent::Codex => Some(AgentKind::Codex),
            PreparedAgent::Claude => Some(AgentKind::Claude),
            PreparedAgent::ClaudeDesktop | PreparedAgent::Pi | PreparedAgent::OpenCode => None,
        };
        let backup = backup_agent.map(|agent| {
            let changes: Vec<_> = self
                .steps
                .iter()
                .filter_map(|step| match step {
                    PreparedStep::ChangeFile { change } => Some(change.clone()),
                    _ => None,
                })
                .collect();
            (agent, changes)
        });

        if let Some((agent, changes)) = &backup {
            BackupManager::create_backup_if_needed(*agent, changes)
                .context("failed to save backup before applying plan")?;
        }

        let mut steps = self.steps.into_iter().peekable();
        while let Some(step) = steps.next() {
            if let PreparedStep::ChangeFile { change } = step {
                let mut changes = vec![change];
                while matches!(steps.peek(), Some(PreparedStep::ChangeFile { .. })) {
                    if let Some(PreparedStep::ChangeFile { change }) = steps.next() {
                        changes.push(change);
                    }
                }

                FileChange::apply_all(&changes).context(
                    "failed to apply prepared files; earlier steps may already be applied",
                )?;
            } else if let PreparedStep::SetClaudeProxyEnabled { token } = step {
                set_claude_enabled(token).context(
                    "failed to update Claude Desktop proxy; earlier steps may already be applied",
                )?;
            } else if let PreparedStep::SetCodexProxyEnabled { token } = step {
                set_codex_enabled(token).context(
                    "failed to update ChatGPT proxy; earlier steps may already be applied",
                )?;
            } else {
                step.apply().context(
                    "failed to apply prepared step; earlier steps may already be applied",
                )?;
            }
        }

        if let Some((agent, changes)) = &backup {
            BackupManager::save_last_written(*agent, changes)
                .context("plan applied, but saving backup state failed")?;
        }
        Ok(())
    }
}

// Fixed operations only; never accept arbitrary shell commands or argument lists.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparedStep {
    /// Saves `codex_proxy_enabled`: the app's proxy token, or `None` to disable.
    SetCodexProxyEnabled {
        token: Option<String>,
    },
    /// Saves `claude_proxy_enabled`: the app's proxy token, or `None` to disable.
    SetClaudeProxyEnabled {
        token: Option<String>,
    },
    ChangeFile {
        change: FileChange,
    },
    PiInstallPlugin {
        executable: PathBuf,
        agent_dir: PathBuf,
    },
    PiRemovePlugin {
        executable: PathBuf,
        agent_dir: PathBuf,
    },
    PiApplyAuth {
        dir: PathBuf,
        change: FileChange,
    },
    #[serde(rename = "opencode_install_plugin")]
    OpenCodeInstallPlugin {
        executable: PathBuf,
        config_dir: PathBuf,
    },
    #[serde(rename = "opencode_verify_plugin")]
    OpenCodeVerifyPlugin {
        config_dir: PathBuf,
    },
    CodexRestartDaemon {
        executable: PathBuf,
        codex_home: PathBuf,
    },
}

#[derive(Serialize)]
pub struct PreparedStepInfo {
    pub description: String,
}

impl PreparedStep {
    // Use PreparedPlan::apply to retain grouped file writes and the agent mutex.
    fn apply(self) -> Result<()> {
        match self {
            Self::SetClaudeProxyEnabled { .. } | Self::SetCodexProxyEnabled { .. } => {
                anyhow::bail!("Desktop proxy requires the GUI")
            }
            Self::ChangeFile { change } => FileChange::apply_all(&[change]),
            Self::PiInstallPlugin {
                executable,
                agent_dir,
            } => pi::install_plugin(&executable, &agent_dir),
            Self::PiRemovePlugin {
                executable,
                agent_dir,
            } => pi::remove_plugin(&executable, &agent_dir),
            Self::PiApplyAuth { dir, change } => pi::apply_auth(&dir, change),
            Self::OpenCodeInstallPlugin {
                executable,
                config_dir,
            } => opencode::install_plugin(&executable, &config_dir),
            Self::OpenCodeVerifyPlugin { config_dir } => opencode::verify_plugin(&config_dir),
            Self::CodexRestartDaemon {
                executable,
                codex_home,
            } => codex::restart_daemon(&executable, &codex_home),
        }
    }
}
