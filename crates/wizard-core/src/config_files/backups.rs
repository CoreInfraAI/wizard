//! Agent-wide backups. No automatic rollback, pending journal, or history pruning.
extern crate alloc;

use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

use super::changes::{FileChange, FileSnapshot};

const BACKUP_VERSION: u32 = 1;
const STATE_VERSION: u32 = 1;

#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    Claude,
}

impl AgentKind {
    fn directory(self) -> Result<PathBuf> {
        let name = match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        };

        Ok(crate::config_dir()?.join("backups").join(name))
    }

    fn ensure_directory(self) -> Result<PathBuf> {
        let directory = self.directory()?;
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        fs::set_permissions(&directory, Permissions::from_mode(0o700))?;
        Ok(directory)
    }
}

pub struct BackupManager;

impl BackupManager {
    pub fn get_backups(agent: AgentKind) -> Result<Vec<Backup>> {
        let directory = agent.directory()?;
        let entries = match fs::read_dir(&directory) {
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
            let Ok(backup) = Backup::load(agent, id) else {
                continue;
            };
            backups.push(backup);
        }
        backups.sort_unstable_by_key(|backup| backup.id);
        backups.reverse();
        Ok(backups)
    }
    pub fn apply_changes(agent: AgentKind, changes: &[FileChange]) -> Result<()> {
        if changes.iter().all(|c| !c.is_changed()) {
            return Ok(());
        }
        let mut state = State::load(agent)?;
        state.create_backup_if_needed(changes)?;
        state.save()?;
        FileChange::apply_all(changes)?;
        state.last_written = Some(
            changes
                .iter()
                .map(|change| (change.path().clone(), change.after.clone()))
                .collect(),
        );
        state.save()?;
        Ok(())
    }
    pub fn load_backup(agent: AgentKind, id: u32) -> Result<()> {
        let backup = Backup::load(agent, id)?;
        let state = State::load(agent)?;

        let expected = state.last_written.as_ref().unwrap_or(&backup.files);
        // TODO: bad
        ensure!(
            expected.keys().eq(backup.files.keys()),
            "backup belongs to a different configuration path set"
        );

        let mut changes = Vec::with_capacity(backup.files.len());
        for (path, snapshot) in &backup.files {
            let mut change = FileChange::read(path.clone())?;
            change.after = snapshot.clone();
            changes.push(change);
        }
        // Restore existing files first; delete files absent from the backup last.
        changes.sort_by_key(|change| matches!(change.after, FileSnapshot::Missing));
        Self::apply_changes(agent, &changes).context("failed to restore backup")
    }
}

// No Debug: last_written may contain credentials.
#[derive(Serialize, Deserialize)]
struct State {
    agent: AgentKind,
    state_version: u32,
    next_backup_id: u32,
    last_written: Option<BTreeMap<PathBuf, FileSnapshot>>,
}

impl State {
    fn default(agent: AgentKind) -> Self {
        Self {
            agent,
            state_version: STATE_VERSION,
            next_backup_id: 1,
            last_written: None,
        }
    }
    fn load(agent: AgentKind) -> Result<Self> {
        let path = agent.directory()?.join("state.json");

        let snapshot = FileSnapshot::read(&path)
            .with_context(|| format!("failed to read backup state {}", path.display()))?;
        let Some(content) = snapshot.content() else {
            return Ok(Self::default(agent));
        };

        let state: Self = serde_json::from_str(content)
            .map_err(|_| anyhow::anyhow!("invalid backup state JSON"))?;
        ensure!(
            state.state_version == STATE_VERSION,
            "unsupported backup state version"
        );
        ensure!(
            state.agent == agent,
            "backup state belongs to a different agent"
        );
        Ok(state)
    }
    fn save(&self) -> Result<()> {
        let mut content = serde_json::to_string_pretty(self)
            .map_err(|_| anyhow::anyhow!("failed to serialize backup state"))?;
        content.push('\n');

        let path = self.agent.ensure_directory()?.join("state.json");

        let mut change = FileChange::read(path)?;
        change.after = FileSnapshot::new(content, 0o600);
        FileChange::apply_all(&[change]).context("failed to save backup state")
    }
    fn get_next_backup_id(&mut self) -> u32 {
        let backup_id = self.next_backup_id;
        self.next_backup_id = self.next_backup_id.strict_add(1);
        backup_id
    }
    fn create_backup_if_needed(&mut self, changes: &[FileChange]) -> Result<()> {
        let current_state: BTreeMap<&PathBuf, &FileSnapshot> = changes
            .iter()
            .map(|change| (change.path(), change.before()))
            .collect();
        let same = compare(&self.last_written, &current_state);
        if same {
            return Ok(());
        }

        let id = self.get_next_backup_id();
        let backup = Backup::from_changes(id, changes);
        backup.save(self.agent)?;
        Ok(())
    }
}

// No Debug: snapshots may contain credentials.
#[derive(Serialize, Deserialize)]
pub struct Backup {
    id: u32,
    backup_version: u32,
    time_created: DateTime<Utc>,
    files: BTreeMap<PathBuf, FileSnapshot>,
}

impl Backup {
    fn from_changes(id: u32, changes: &[FileChange]) -> Self {
        Self {
            id,
            backup_version: BACKUP_VERSION,
            time_created: Utc::now(),
            files: changes
                .iter()
                .map(|change| (change.path().clone(), change.before().clone()))
                .collect(),
        }
    }
    fn load(agent: AgentKind, id: u32) -> Result<Self> {
        let directory = agent.directory()?;

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
            backup.backup_version == BACKUP_VERSION,
            "unsupported backup version"
        );
        Ok(backup)
    }

    fn save(&self, agent: AgentKind) -> Result<()> {
        let mut content = serde_json::to_string_pretty(self)
            .map_err(|_| anyhow::anyhow!("failed to serialize backup"))?;
        content.push('\n');

        let directory = agent.ensure_directory()?;

        let mut change = FileChange::read(directory.join(format!("{}.json", self.id)))?;
        change.after = FileSnapshot::new(content, 0o600);
        FileChange::apply_all(&[change]).context("failed to save backup")
    }
}

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
