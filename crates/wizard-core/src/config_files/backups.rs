//! Agent-wide backups. No automatic rollback, pending journal, or history pruning.
extern crate alloc;

use super::changes::{FileChange, FileSnapshot};
use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

const BACKUP_VERSION: u32 = 1;
const STATE_VERSION: u32 = 1;

#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    Claude,
    ClaudeDesktop,
}

impl AgentKind {
    /// Only the agent's own configuration files. Wizard also edits app-owned state in place
    /// (`~/.claude.json`, `ChatGPT`'s Preferences): their apps rewrite it constantly, and a
    /// restore would roll back unrelated state such as MCP servers or a proxy Wizard removed.
    pub(crate) fn backs_up(self, path: &Path) -> bool {
        if self == Self::ClaudeDesktop {
            let Some(parent) = path.parent() else {
                return false;
            };
            let name = path.file_name().and_then(|name| name.to_str());
            if parent.file_name().is_some_and(|name| name == "Claude-3p") {
                return name == Some("claude_desktop_config.json");
            }
            return parent
                .file_name()
                .is_some_and(|name| name == "configLibrary")
                && parent
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "Claude-3p")
                && name.is_some_and(|name| {
                    name == "_meta.json"
                        || name.strip_suffix(".json").is_some_and(|id| {
                            uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
                        })
                });
        }
        let names: &[&str] = match self {
            Self::Codex => &["config.toml", ".env"],
            Self::Claude => &["settings.json", "no-proxy.sh"],
            Self::ClaudeDesktop => unreachable!(),
        };
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| names.contains(&name))
    }

    fn directory(self) -> Result<PathBuf> {
        let name = match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::ClaudeDesktop => "claude_desktop",
        };

        Ok(wizard_settings::paths::config_dir()?
            .join("backups")
            .join(name))
    }
}

pub struct BackupManager {
    agent: AgentKind,
    directory: PathBuf,
}

impl BackupManager {
    pub fn new(agent: AgentKind) -> Result<Self> {
        Ok(Self {
            agent,
            directory: agent.directory()?,
        })
    }

    #[cfg(test)]
    pub(crate) fn in_directory(agent: AgentKind, directory: PathBuf) -> Self {
        Self { agent, directory }
    }

    fn ensure_directory(&self) -> Result<&Path> {
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        fs::set_permissions(&self.directory, Permissions::from_mode(0o700))?;
        Ok(&self.directory)
    }

    pub fn get_backups(&self) -> Result<Vec<Backup>> {
        let directory = &self.directory;
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("failed to list backups"),
        };

        let mut backups = Vec::new();
        for entry in entries {
            // TODO: maybe return Vec<Result>?
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };

            let Some(id) = name
                .strip_suffix(".json")
                .and_then(|id| id.parse::<u32>().ok())
            else {
                continue;
            };
            if name != format!("{id}.json") {
                continue;
            }
            let Ok(backup) = Backup::load(&self.directory, id) else {
                continue;
            };
            backups.push(backup);
        }
        backups.sort_unstable_by_key(|backup| backup.id);
        backups.reverse();
        Ok(backups)
    }

    pub fn apply_changes(&self, changes: &[FileChange]) -> Result<()> {
        self.create_backup_if_needed(changes)?;

        FileChange::apply_all(changes)?;

        self.save_last_written(changes)
    }

    pub fn create_backup_if_needed(&self, changes: &[FileChange]) -> Result<()> {
        if changes.iter().all(|change| !change.is_changed()) {
            return Ok(());
        }

        State::load(self)?.create_backup_if_needed(self, changes)
    }

    pub fn save_last_written(&self, changes: &[FileChange]) -> Result<()> {
        if changes.iter().all(|change| !change.is_changed()) {
            return Ok(());
        }

        let mut state = State::load(self)?;
        state.last_written = Some(
            changes
                .iter()
                .map(|change| (change.path().clone(), change.after.clone()))
                .collect(),
        );
        state.save(self)
    }

    pub fn load_backup(&self, id: u32) -> Result<()> {
        self.apply_changes(&self.restore_changes(id)?)
            .context("failed to restore backup")
    }

    pub(crate) fn restore_changes(&self, id: u32) -> Result<Vec<FileChange>> {
        let backup = Backup::load(&self.directory, id)?;

        let mut changes = Vec::with_capacity(backup.files.len());
        // Older backups may also hold app-owned state files.
        for (path, snapshot) in backup
            .files
            .iter()
            .filter(|(path, _)| self.agent.backs_up(path))
        {
            let mut change = FileChange::read(path.clone())?;
            change.after = snapshot.clone();
            changes.push(change);
        }
        // Desktop restores keep the profile, selection and deployment mode together.
        // Other agents back up only what the restore changes.
        if self.agent != AgentKind::ClaudeDesktop {
            changes.retain(FileChange::is_changed);
        }
        // Restore existing files first; delete files absent from the backup last.
        changes.sort_by_key(|change| matches!(change.after, FileSnapshot::Missing));
        Ok(changes)
    }
}

// No Debug: last_written may contain credentials.
#[derive(Serialize, Deserialize)]
struct State {
    agent: AgentKind,
    version: u32,
    next_backup_id: u32,
    last_written: Option<BTreeMap<PathBuf, FileSnapshot>>,
}

impl State {
    fn default(agent: AgentKind) -> Self {
        Self {
            agent,
            version: STATE_VERSION,
            next_backup_id: 1,
            last_written: None,
        }
    }
    fn load(manager: &BackupManager) -> Result<Self> {
        let path = manager.directory.join("state.json");

        let snapshot = FileSnapshot::read(&path)
            .with_context(|| format!("failed to read backup state {}", path.display()))?;
        let Some(content) = snapshot.content() else {
            return Ok(Self::default(manager.agent));
        };

        let state: Self = serde_json::from_str(content)
            .map_err(|_| anyhow::anyhow!("invalid backup state JSON"))?;
        ensure!(
            state.version == STATE_VERSION,
            "unsupported backup state version"
        );
        ensure!(
            state.agent == manager.agent,
            "backup state belongs to a different agent"
        );
        Ok(state)
    }
    fn save(&self, manager: &BackupManager) -> Result<()> {
        let mut content = serde_json::to_string_pretty(self)
            .map_err(|_| anyhow::anyhow!("failed to serialize backup state"))?;
        content.push('\n');

        let path = manager.ensure_directory()?.join("state.json");

        let mut change = FileChange::read(path)?;
        change.after = FileSnapshot::new(content, 0o600);
        FileChange::apply_all(&[change]).context("failed to save backup state")
    }
    fn get_next_backup_id(&mut self) -> u32 {
        let backup_id = self.next_backup_id;
        self.next_backup_id = self.next_backup_id.strict_add(1);
        backup_id
    }
    fn create_backup_if_needed(
        &mut self,
        manager: &BackupManager,
        changes: &[FileChange],
    ) -> Result<()> {
        // A path may occur in multiple steps; keep its first original snapshot.
        let current_state: BTreeMap<&PathBuf, &FileSnapshot> = changes
            .iter()
            .rev()
            .map(|change| (change.path(), change.before()))
            .collect();
        let same = compare(&self.last_written, &current_state);
        if same {
            return Ok(());
        }

        // Save the next ID first, so a failed state save can never reuse an existing backup's ID.
        let id = self.get_next_backup_id();
        self.save(manager)?;
        Backup::from_changes(id, changes).save(manager)
    }
}

// No Debug: snapshots may contain credentials.
#[derive(Serialize, Deserialize)]
pub struct Backup {
    id: u32,
    version: u32,
    time_created: DateTime<Utc>,
    files: BTreeMap<PathBuf, FileSnapshot>,
}

impl Backup {
    fn from_changes(id: u32, changes: &[FileChange]) -> Self {
        Self {
            id,
            version: BACKUP_VERSION,
            time_created: Utc::now(),
            files: changes
                .iter()
                .rev()
                .map(|change| (change.path().clone(), change.before().clone()))
                .collect(),
        }
    }
    fn load(directory: &Path, id: u32) -> Result<Self> {
        let path = directory.join(format!("{id}.json"));
        let snapshot = FileSnapshot::read(&path)
            .with_context(|| format!("failed to read backup {}", path.display()))?;
        let content = snapshot.content().context("backup file is missing")?;
        // Parser errors may contain credentials. Never retain their source.
        let backup: Self =
            serde_json::from_str(content).map_err(|_| anyhow::anyhow!("invalid backup JSON"))?;
        ensure!(
            backup.id == id,
            "backup identifier does not match its filename"
        );
        ensure!(
            backup.version == BACKUP_VERSION,
            "unsupported backup version"
        );
        Ok(backup)
    }

    fn save(&self, manager: &BackupManager) -> Result<()> {
        let mut content = serde_json::to_string_pretty(self)
            .map_err(|_| anyhow::anyhow!("failed to serialize backup"))?;
        content.push('\n');

        let directory = manager.ensure_directory()?;

        let mut change = FileChange::read(directory.join(format!("{}.json", self.id)))?;
        // Another Wizard process may have used this ID; never replace its backup.
        ensure!(
            matches!(change.before(), FileSnapshot::Missing),
            "backup {} already exists",
            self.id
        );
        change.after = FileSnapshot::new(content, 0o600);
        FileChange::apply_all(&[change]).context("failed to save backup")
    }
}

#[expect(clippy::ref_option)]
fn compare<A: PartialEq, B: PartialEq>(
    last: &Option<BTreeMap<A, B>>,
    current: &BTreeMap<&A, &B>,
) -> bool {
    let Some(last) = last else {
        return false;
    };
    let current_iter = current.iter().map(|(&k, &v)| (k, v));
    last.iter().eq(current_iter)
}

#[cfg(test)]
mod tests {
    use super::AgentKind;
    use std::path::Path;

    #[test]
    fn backups_cover_only_agent_configuration() {
        assert!(AgentKind::Codex.backs_up(Path::new("/home/.codex/config.toml")));
        assert!(AgentKind::Codex.backs_up(Path::new("/home/.codex/.env")));
        assert!(!AgentKind::Codex.backs_up(Path::new("/config/Codex/Default/Preferences")));
        assert!(AgentKind::Claude.backs_up(Path::new("/home/.claude/settings.json")));
        assert!(AgentKind::Claude.backs_up(Path::new("/home/.claude/no-proxy.sh")));
        assert!(!AgentKind::Claude.backs_up(Path::new("/home/.claude.json")));
    }
}
