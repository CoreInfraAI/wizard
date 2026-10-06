use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

use super::changes::FileSnapshot;

pub(crate) fn read(path: &Path) -> Result<Value> {
    let snapshot = FileSnapshot::read(path).context("failed to read JSON file")?;
    parse(snapshot.as_ref())
}

/// Missing files are empty objects. Never expose JSON contents in parser errors.
pub(crate) fn parse(snapshot: Option<&FileSnapshot>) -> Result<Value> {
    let Some(snapshot) = snapshot else {
        return Ok(json!({}));
    };
    let value: Value = serde_json::from_str(&snapshot.content)
        .map_err(|_| anyhow::anyhow!("invalid JSON file"))?;
    ensure!(value.is_object(), "JSON file must contain an object");
    Ok(value)
}

/// Edits only the working snapshot. A semantic no-op preserves the original text.
pub(crate) fn update(
    after: &mut Option<FileSnapshot>,
    edit: impl FnOnce(&mut Value) -> Result<()>,
) -> Result<()> {
    let original = parse(after.as_ref())?;
    let mut updated = original.clone();
    edit(&mut updated)?;
    ensure!(updated.is_object(), "JSON file must contain an object");
    if updated == original {
        return Ok(());
    }
    let mut content = serde_json::to_string_pretty(&updated)
        .map_err(|_| anyhow::anyhow!("failed to serialize JSON"))?;
    content.push('\n');
    *after = Some(FileSnapshot::new(content, 0o600));
    Ok(())
}
