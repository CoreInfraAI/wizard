use std::{fs, io::Write as _, path::Path};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

/// Missing files are empty objects. Never expose JSON contents in parser errors.
pub(crate) fn read(path: &Path) -> Result<Value> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(!metadata.file_type().is_symlink(), "JSON file is a symlink"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error).context("failed to inspect JSON file"),
    }
    let bytes = fs::read(path).context("failed to read JSON file")?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid JSON file"))?;
    ensure!(value.is_object(), "JSON file must contain an object");
    Ok(value)
}

/// Callers serialize operations. Each file is replaced atomically, not multiple files together.
pub(crate) fn write(path: &Path, original: &Value, updated: &Value) -> Result<()> {
    if original == updated {
        return Ok(());
    }
    let parent = path.parent().context("JSON file has no parent directory")?;
    fs::create_dir_all(parent).context("failed to create JSON directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("failed to create temporary JSON file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to restrict JSON file permissions")?;
    }
    serde_json::to_writer_pretty(&mut temporary, updated)
        .map_err(|_| anyhow::anyhow!("failed to serialize JSON"))?;
    temporary
        .write_all(b"\n")
        .context("failed to write JSON file")?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync JSON file")?;
    temporary
        .persist(path)
        .context("failed to save JSON file")?;
    Ok(())
}
