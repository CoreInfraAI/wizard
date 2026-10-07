#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use tempfile::NamedTempFile;

// No Debug: file contents may contain credentials.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum FileSnapshot {
    Missing,
    Present {
        content: String,
        #[cfg(unix)]
        #[serde(with = "octal_mode")]
        mode: u32,
    },
}

#[cfg(unix)]
mod octal_mode {
    use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde field serializer signature"
    )]
    pub fn serialize<S: Serializer>(mode: &u32, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{mode:04o}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 4 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
            return Err(D::Error::custom("invalid Unix mode"));
        }
        u32::from_str_radix(&value, 8).map_err(|_| D::Error::custom("invalid Unix mode"))
    }
}

impl FileSnapshot {
    pub(crate) fn new(content: String, unix_mode: u32) -> Self {
        #[cfg(not(unix))]
        let _ = unix_mode;
        Self::Present {
            content,
            #[cfg(unix)]
            mode: unix_mode,
        }
    }

    pub(crate) fn content(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Present { content, .. } => Some(content),
        }
    }

    /// Reads the current UTF-8 file and its permissions without changing it.
    /// A missing file is distinct from an existing empty file.
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Missing),
            Err(error) => return Err(error).context("failed to inspect file"),
        };
        ensure!(
            !metadata.file_type().is_symlink(),
            "file is a symlink; refusing to read it"
        );
        ensure!(metadata.is_file(), "file must be a regular file");

        let mut file = fs::File::open(path).context("failed to open file")?;
        #[cfg(unix)]
        let mode = file
            .metadata()
            .context("failed to read file permissions")?
            .permissions()
            .mode()
            & 0o7777;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .context("failed to read UTF-8 file")?;
        Ok(Self::Present {
            content,
            #[cfg(unix)]
            mode,
        })
    }

    fn crate_temp_file_for(&self, path: &Path) -> Result<Option<NamedTempFile>> {
        let Self::Present {
            content,
            #[cfg(unix)]
            mode,
        } = self
        else {
            return Ok(None);
        };
        let parent = path
            .parent()
            .context("config file has no parent directory")?;
        fs::create_dir_all(parent).context("failed to create config directory")?;
        let mut temporary = NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to prepare {}", path.display()))?;
        temporary
            .write_all(content.as_bytes())
            .context("failed to write temporary config file")?;
        #[cfg(unix)]
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(*mode))
            .context("failed to set config permissions")?;
        temporary
            .as_file()
            .sync_all()
            .context("failed to sync temporary config file")?;
        Ok(Some(temporary))
    }
}

// Keep the original snapshot and path immutable outside this module.
// No Debug: snapshots may contain credentials.
pub(crate) struct FileChange {
    path: PathBuf,
    before: FileSnapshot,
    pub after: FileSnapshot,
}

impl FileChange {
    pub(crate) fn read(path: PathBuf) -> Result<Self> {
        let path = std::path::absolute(path).context("failed to resolve config path")?;
        let before = FileSnapshot::read(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Ok(Self {
            path,
            after: before.clone(),
            before,
        })
    }

    pub(crate) fn is_changed(&self) -> bool {
        self.before != self.after
    }

    fn verify_before(&self) -> Result<()> {
        let current = FileSnapshot::read(&self.path)
            .with_context(|| format!("failed to reread {}", self.path.display()))?;
        ensure!(
            current == self.before,
            "{} changed during the operation; please retry",
            self.path.display()
        );
        Ok(())
    }

    /// Stages all writes before replacing any target. Each replacement is atomic,
    /// but the batch is not: there is no rollback or backup. Callers serialize the
    /// entire read/prepare/apply operation. Checks cannot exclude external writers.
    /// Unchanged snapshots are skipped without any filesystem operations.
    pub(crate) fn apply_all(changes: &[Self]) -> Result<()> {
        let changes: Vec<_> = changes
            .iter()
            .filter(|change| change.is_changed())
            .collect();
        for (index, change) in changes.iter().enumerate() {
            ensure!(
                !changes[..index]
                    .iter()
                    .any(|other| other.path == change.path),
                "duplicate path: {}",
                change.path.display()
            );
        }

        let mut prepared = Vec::with_capacity(changes.len());
        for change in &changes {
            let temporary = change.after.crate_temp_file_for(&change.path)?;
            prepared.push(temporary);
        }

        // Catch changes during staging before applying any member of the batch.
        for change in &changes {
            change.verify_before()?;
        }
        for (change, temporary) in changes.into_iter().zip(prepared) {
            change.apply_prepared(temporary).with_context(|| {
                format!(
                    "failed to apply {}; earlier file changes may already be applied",
                    change.path.display()
                )
            })?;
        }
        Ok(())
    }

    fn apply_prepared(&self, temporary: Option<NamedTempFile>) -> Result<()> {
        self.verify_before()?;
        match temporary {
            Some(temporary) => {
                if matches!(self.before, FileSnapshot::Missing) {
                    // Do not overwrite a file created after the last check.
                    temporary
                        .persist_noclobber(&self.path)
                        .context("failed to create config file")?;
                } else {
                    temporary
                        .persist(&self.path)
                        .context("failed to replace config file")?;
                }
            }
            None => fs::remove_file(&self.path).context("failed to remove config file")?,
        }
        Ok(())
    }
}
