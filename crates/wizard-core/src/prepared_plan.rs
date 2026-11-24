//! Serializable prepare / confirm / apply contract shared by CLI and GUI.
//! Only execute plans from trusted preparation: serialization is not authorization.
//! Snapshots contain credentials and must never be logged or printed as a plan preview.
//! External commands have no rollback; `@latest` does not pin a package version.

use crate::{
    agents::{
        claude, claude_desktop, codex,
        desktop_install::{self, DesktopApp},
        opencode, pi,
    },
    config_files::{
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::MutexGuard,
};

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

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesktopRestart {
    app: DesktopApp,
    last_path: Option<PathBuf>,
}

// No Debug: the original and prepared contents may contain credentials.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPlan {
    agent: PreparedAgent,
    pub(crate) steps: Vec<PreparedStep>,
    #[serde(default)]
    desktop: Option<DesktopRestart>,
}

impl PreparedPlan {
    pub(crate) fn new(agent: PreparedAgent, steps: Vec<PreparedStep>) -> Self {
        Self {
            agent,
            steps,
            desktop: None,
        }
    }

    /// GUI configuration closes the desktop app before writing and opens it afterwards.
    pub fn restart_desktop(&mut self, app: DesktopApp, last_path: Option<&Path>) {
        self.desktop = Some(DesktopRestart {
            app,
            last_path: last_path.map(Path::to_owned),
        });
    }

    #[must_use]
    pub fn restarts_desktop(&self) -> bool {
        self.desktop.is_some()
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
        if let Some(desktop) = &self.desktop {
            info.push(PreparedStepInfo {
                description: format!(
                    "Применит настройки при закрытом {}. Запустите его после применения.",
                    desktop.app.product()
                ),
            });
        }

        for step in &self.steps {
            let item = match step {
                PreparedStep::ChangeFile { change } | PreparedStep::PiApplyAuth { change, .. }
                    if !change.is_changed() => {
                    continue
                },
                PreparedStep::ChangeFile { change } => {
                    let action = match (&change.after, change.before()) {
                        (FileSnapshot::Missing, _) => "Удалит файл",
                        (_, FileSnapshot::Missing) => "Создаст файл",
                        _ => "Изменит файл",
                    };
                    PreparedStepInfo {
                        description: format!("{action} {}", change.path().display()),
                    }
                },
                PreparedStep::PiApplyAuth { change, .. } => PreparedStepInfo {
                    description: format!("Запишет настройки доступа Pi в {}.", change.path().display()),
                },
                PreparedStep::PiInstallPlugin { .. } => PreparedStepInfo {
                    description: "Установит последнюю версию плагина CoreInfra для Pi.".to_owned(),
                },
                PreparedStep::PiRemovePlugin { .. } => PreparedStepInfo {
                    description: "Удалит плагин CoreInfra из Pi.".to_owned(),
                },
                PreparedStep::OpenCodeInstallPlugin { .. } | PreparedStep::OpenCodeAddPlugin { .. } => PreparedStepInfo {
                    description: "Установит последнюю версию плагина CoreInfra для OpenCode.".to_owned(),
                },
                PreparedStep::OpenCodeVerifyPlugin { .. } => PreparedStepInfo {
                    description: "Убедится, что плагин CoreInfra добавлен в конфигурацию OpenCode.".to_owned(),
                },
                PreparedStep::OpenCodeSetCredential { .. } => PreparedStepInfo {
                    description: "Сохранит ключ CoreInfra от Wizard в OpenCode и сделает его активным.".to_owned(),
                },
                PreparedStep::OpenCodeReload { .. } => PreparedStepInfo {
                    description: "Перезагрузит конфигурацию фоновой службы OpenCode, если она запущена.".to_owned(),
                },
                PreparedStep::OpenCodeRemoveCredential { .. } => PreparedStepInfo {
                    description: "Удалит ключ CoreInfra от Wizard из OpenCode. Другие ключи CoreInfra сохранятся.".to_owned(),
                },
                PreparedStep::SetChatGptProxy { enabled, .. } => PreparedStepInfo {
                    description: if *enabled {
                        "Настроит подключение ChatGPT через локальный прокси Wizard."
                    } else {
                        "Удалит настройки прокси Wizard из ChatGPT."
                    }.to_owned(),
                },
                PreparedStep::SetClaudeDesktop1pMedia { enabled, .. } => PreparedStepInfo {
                    description: if *enabled {
                        "Зарегистрирует локальный MCP-сервер coreinfra_media в Claude Desktop."
                    } else {
                        "Удалит локальный MCP-сервер coreinfra_media из настроек Claude Desktop."
                    }.to_owned(),
                },
                PreparedStep::SetCodexProxyEnabled { enabled } => PreparedStepInfo {
                    description: if *enabled {
                        "Включит локальный прокси ChatGPT в Wizard."
                    } else {
                        "Выключит локальный прокси ChatGPT в Wizard."
                    }.to_owned(),
                },
                PreparedStep::SetClaudeProxyEnabled { enabled } => PreparedStepInfo {
                    description: if *enabled {
                        "Включит локальный прокси Claude Desktop в Wizard."
                    } else {
                        "Выключит локальный прокси Claude Desktop в Wizard."
                    }.to_owned(),
                },
                PreparedStep::CodexRestartDaemon { .. } => PreparedStepInfo {
                    description: "Перезапустит фоновую службу Codex, если она запущена. Это может прервать текущую работу.".to_owned(),
                },
                PreparedStep::SetChatGptUpdatesDisabled { disabled } => PreparedStepInfo {
                    description: if *disabled {
                        "Отключит встроенные обновления ChatGPT: Wizard добавит объект входа, который задаёт CODEX_SPARKLE_ENABLED=false. Подействует после перезапуска ChatGPT; обновляйте ChatGPT в Wizard."
                    } else {
                        "Включит встроенные обновления ChatGPT: Wizard удалит свой объект входа. Подействует после перезапуска ChatGPT."
                    }.to_owned(),
                },
            };
            info.push(item);
        }
        info
    }

    /// Call from a Tokio blocking worker. Adjacent file changes are applied as a group.
    /// Agent backups cover the entire plan; there is no rollback on failure.
    pub fn apply(self) -> Result<()> {
        anyhow::ensure!(
            self.desktop.is_none()
                && !self.steps.iter().any(|step| matches!(
                    step,
                    PreparedStep::SetClaudeProxyEnabled { .. }
                        | PreparedStep::SetCodexProxyEnabled { .. }
                        | PreparedStep::SetChatGptProxy { .. }
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
    /// The caller validates account context and keeps it stable until this method
    /// returns. Proxy callbacks receive only demand.
    pub fn apply_with_proxies(
        self,
        set_claude_enabled: impl FnMut(bool) -> Result<()>,
        set_codex_enabled: impl FnMut(bool) -> Result<()>,
    ) -> Result<()> {
        let backup = self.backup_agent().map(BackupManager::new).transpose()?;
        self.apply_with_backup(backup, set_claude_enabled, set_codex_enabled)
    }

    fn backup_agent(&self) -> Option<AgentKind> {
        match self.agent {
            PreparedAgent::Codex => Some(AgentKind::Codex),
            PreparedAgent::Claude => Some(AgentKind::Claude),
            PreparedAgent::ClaudeDesktop => Some(AgentKind::ClaudeDesktop),
            PreparedAgent::Pi | PreparedAgent::OpenCode => None,
        }
    }

    pub(crate) fn apply_with_backup(
        mut self,
        manager: Option<BackupManager>,
        set_claude_enabled: impl FnMut(bool) -> Result<()>,
        set_codex_enabled: impl FnMut(bool) -> Result<()>,
    ) -> Result<()> {
        let _guard = self.agent.lock()?;

        if let Some(desktop) = self.desktop.take() {
            desktop_install::with_closed(desktop.app, desktop.last_path.as_deref(), || {
                self.apply_closed(manager, set_claude_enabled, set_codex_enabled)
            })
        } else {
            self.apply_closed(manager, set_claude_enabled, set_codex_enabled)
        }
    }

    fn apply_closed(
        mut self,
        manager: Option<BackupManager>,
        mut set_claude_enabled: impl FnMut(bool) -> Result<()>,
        mut set_codex_enabled: impl FnMut(bool) -> Result<()>,
    ) -> Result<()> {
        // Only ChatGPT's app-owned Preferences are read after shutdown. CLI configuration
        // and Claude profiles retain their original snapshots and conflict checks.
        for step in &mut self.steps {
            if let PreparedStep::SetChatGptProxy { path, enabled } = step {
                *step = PreparedStep::ChangeFile {
                    change: codex::prepare_desktop_proxy(path.clone(), *enabled)?,
                };
            } else if let PreparedStep::SetClaudeDesktop1pMedia {
                path,
                enabled,
                headers,
            } = step
            {
                *step = PreparedStep::ChangeFile {
                    change: claude_desktop::prepare_1p_media(
                        path.clone(),
                        *enabled,
                        headers.clone(),
                    )?,
                };
            }
        }

        // Refuse a write (symlink, file changed since preparation) before an install step runs.
        // Each write checks again.
        for step in &self.steps {
            if let PreparedStep::ChangeFile { change } | PreparedStep::PiApplyAuth { change, .. } =
                step
                && (change.is_changed() || matches!(self.agent, PreparedAgent::ClaudeDesktop))
            {
                change.verify_before()?;
            }
        }

        let backup = self.backup_agent().zip(manager).map(|(agent, manager)| {
            let changes: Vec<_> = self
                .steps
                .iter()
                // Desktop needs the profile, selection and deployment mode together, even
                // when only its token changes. Other agents back up only changed files.
                .filter_map(|step| match step {
                    PreparedStep::ChangeFile { change }
                        if (change.is_changed() || agent == AgentKind::ClaudeDesktop)
                            && agent.backs_up(change.path()) =>
                    {
                        Some(change.clone())
                    }
                    _ => None,
                })
                .collect();
            (manager, changes)
        });

        if let Some((manager, changes)) = &backup {
            manager
                .create_backup_if_needed(changes)
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
            } else if let PreparedStep::SetClaudeProxyEnabled { enabled } = step {
                set_claude_enabled(enabled).context(
                    "failed to update Claude Desktop proxy; earlier steps may already be applied",
                )?;
            } else if let PreparedStep::SetCodexProxyEnabled { enabled } = step {
                set_codex_enabled(enabled).context(
                    "failed to update ChatGPT proxy; earlier steps may already be applied",
                )?;
            } else {
                step.apply().context(
                    "failed to apply prepared step; earlier steps may already be applied",
                )?;
            }
        }

        if let Some((manager, changes)) = &backup {
            manager
                .save_last_written(changes)
                .context("plan applied, but saving backup state failed")?;
        }
        Ok(())
    }
}

// Fixed operations only; never accept arbitrary shell commands or argument lists.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparedStep {
    /// Read the application-owned Preferences after shutdown; preserve unrelated fields.
    #[serde(rename = "set_chatgpt_proxy")]
    SetChatGptProxy {
        path: PathBuf,
        enabled: bool,
    },
    /// Read Claude Desktop's 1p configuration after shutdown; change only the
    /// Wizard-owned `coreinfra_media` server.
    #[serde(rename = "set_claude_desktop_1p_media")]
    SetClaudeDesktop1pMedia {
        path: PathBuf,
        enabled: bool,
        /// The grant file the registration passes to the connector.
        headers: PathBuf,
    },
    /// Saves `ChatGPT`'s demand for the local proxy, without an agent credential.
    SetCodexProxyEnabled {
        enabled: bool,
    },
    /// Saves Claude Desktop's demand for the local proxy.
    SetClaudeProxyEnabled {
        enabled: bool,
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
    /// `OpenCode` 2 `plugin add`.
    #[serde(rename = "opencode_add_plugin")]
    OpenCodeAddPlugin {
        executable: PathBuf,
        config_dir: PathBuf,
    },
    #[serde(rename = "opencode_verify_plugin")]
    OpenCodeVerifyPlugin {
        config_dir: PathBuf,
    },
    #[serde(rename = "opencode_set_credential")]
    OpenCodeSetCredential {
        executable: PathBuf,
        token: String,
    },
    #[serde(rename = "opencode_remove_credential")]
    OpenCodeRemoveCredential {
        executable: PathBuf,
    },
    #[serde(rename = "opencode_reload")]
    OpenCodeReload {
        executable: PathBuf,
    },
    CodexRestartDaemon {
        executable: PathBuf,
        codex_home: PathBuf,
    },
    /// macOS: turns `ChatGPT`'s own updater off or back on for its next launch.
    #[serde(rename = "set_chatgpt_updates_disabled")]
    SetChatGptUpdatesDisabled {
        disabled: bool,
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
            Self::SetChatGptProxy { .. } => {
                anyhow::bail!("ChatGPT Preferences must be prepared after closing the app")
            }
            Self::SetClaudeDesktop1pMedia { .. } => {
                anyhow::bail!("Claude Desktop configuration must be prepared after closing the app")
            }
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
            Self::OpenCodeAddPlugin {
                executable,
                config_dir,
            } => opencode::add_plugin(&executable, &config_dir),
            Self::OpenCodeVerifyPlugin { config_dir } => opencode::verify_plugin(&config_dir),
            Self::OpenCodeSetCredential { executable, token } => {
                opencode::set_credential(&executable, &token)
            }
            Self::OpenCodeRemoveCredential { executable } => {
                opencode::remove_credential(&executable)
            }
            Self::OpenCodeReload { executable } => opencode::reload(&executable),
            Self::CodexRestartDaemon {
                executable,
                codex_home,
            } => codex::restart_daemon(&executable, &codex_home),
            #[cfg(target_os = "macos")]
            Self::SetChatGptUpdatesDisabled { disabled } => {
                crate::agents::chatgpt_updates::set_disabled(disabled)
            }
            #[cfg(not(target_os = "macos"))]
            Self::SetChatGptUpdatesDisabled { .. } => {
                anyhow::bail!("ChatGPT updates are managed this way only on macOS")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_files::json;

    #[test]
    fn chatgpt_profile_is_created_or_merged_after_shutdown_before_proxy_start() -> Result<()> {
        for initialized_during_shutdown in [false, true] {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join("Default/Preferences");
            let plan = PreparedPlan::new(
                PreparedAgent::Codex,
                vec![
                    PreparedStep::SetChatGptProxy {
                        path: path.clone(),
                        enabled: true,
                    },
                    PreparedStep::SetCodexProxyEnabled { enabled: true },
                ],
            );
            assert!(!plan.info().is_empty());
            assert!(!path.parent().expect("profile directory").exists());
            if initialized_during_shutdown {
                std::fs::create_dir_all(path.parent().expect("profile directory"))?;
                std::fs::write(&path, r#"{"profile":{"name":"saved during shutdown"}}"#)?;
            }
            let mut activated = false;
            plan.apply_with_backup(
                None,
                |_| panic!("unexpected Claude proxy"),
                |enabled| {
                    assert!(enabled);
                    let config = json::read(&path)?;
                    assert_eq!(config["proxy"]["server"], "http://127.0.0.1:33788");
                    if initialized_during_shutdown {
                        assert_eq!(config["profile"]["name"], "saved during shutdown");
                    } else {
                        assert_eq!(config.as_object().expect("preferences object").len(), 1);
                    }
                    activated = true;
                    Ok(())
                },
            )?;
            assert!(activated);
        }
        Ok(())
    }

    #[test]
    fn desktop_profile_does_not_bypass_existing_configuration_conflict_checks() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let config = directory.path().join("config.toml");
        std::fs::write(&config, "original")?;
        let mut change = FileChange::read(config.clone())?;
        change.after = FileSnapshot::new("prepared".into(), 0o600);
        let preferences = directory.path().join("Default/Preferences");
        let plan = PreparedPlan::new(
            PreparedAgent::Codex,
            vec![
                PreparedStep::SetChatGptProxy {
                    path: preferences.clone(),
                    enabled: true,
                },
                PreparedStep::ChangeFile { change },
                PreparedStep::SetCodexProxyEnabled { enabled: true },
            ],
        );
        std::fs::write(&config, "user change after preparation")?;
        assert!(
            plan.apply_with_backup(
                None,
                |_| panic!("unexpected Claude proxy"),
                |_| panic!("proxy must not start after failed preflight"),
            )
            .is_err()
        );
        assert!(!preferences.exists());
        assert_eq!(
            std::fs::read_to_string(config)?,
            "user change after preparation"
        );
        Ok(())
    }
}
