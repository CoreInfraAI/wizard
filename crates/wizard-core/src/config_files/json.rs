use super::changes::FileSnapshot;
use anyhow::{Context as _, Result, ensure};
use jsonc_parser::{
    ParseOptions,
    cst::{CstObject, CstRootNode},
};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn read(path: &Path) -> Result<Value> {
    let snapshot = FileSnapshot::read(path).context("failed to read JSON file")?;
    parse(&snapshot)
}

/// Missing files are empty objects. Never expose JSON contents in parser errors.
pub(crate) fn parse(snapshot: &FileSnapshot) -> Result<Value> {
    let FileSnapshot::Present { content, .. } = snapshot else {
        return Ok(json!({}));
    };
    let value: Value =
        serde_json::from_str(content).map_err(|_| anyhow::anyhow!("invalid JSON file"))?;
    ensure!(value.is_object(), "JSON file must contain an object");
    Ok(value)
}

/// Edits only the working snapshot. A semantic no-op preserves the original text.
pub(crate) fn update(
    after: &mut FileSnapshot,
    edit: impl FnOnce(&mut Value) -> Result<()>,
) -> Result<()> {
    let original = parse(after)?;
    let mut updated = original.clone();
    edit(&mut updated)?;
    ensure!(updated.is_object(), "JSON file must contain an object");
    if updated == original {
        return Ok(());
    }
    let mut content = serde_json::to_string_pretty(&updated)
        .map_err(|_| anyhow::anyhow!("failed to serialize JSON"))?;
    content.push('\n');
    *after = FileSnapshot::new(content, 0o600);
    Ok(())
}

/// JSONC as `OpenCode` accepts it: JSON with comments and trailing commas, no other JSON5.
fn jsonc_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

/// Parses an object from text, never exposing its contents in errors.
fn parse_jsonc_root(content: &str) -> Result<(CstRootNode, CstObject)> {
    let root = CstRootNode::parse(content, &jsonc_options())
        .map_err(|_| anyhow::anyhow!("invalid JSONC file"))?;
    let object = root
        .object_value()
        .context("JSONC file must contain an object")?;
    Ok((root, object))
}

/// Missing files are empty objects. Never expose JSONC contents in parser errors.
pub(crate) fn parse_jsonc(snapshot: &FileSnapshot) -> Result<Value> {
    let FileSnapshot::Present { content, .. } = snapshot else {
        return Ok(json!({}));
    };
    let (_, object) = parse_jsonc_root(content)?;
    object
        .to_serde_value()
        .context("failed to convert JSONC file")
}

/// Edits the working snapshot's syntax tree, preserving comments and formatting.
/// Missing files stay missing. A semantic no-op preserves the original text.
pub(crate) fn update_jsonc(
    after: &mut FileSnapshot,
    edit: impl FnOnce(&CstObject) -> Result<()>,
) -> Result<()> {
    let FileSnapshot::Present { content, .. } = after else {
        return Ok(());
    };
    let (root, object) = parse_jsonc_root(content)?;
    let original = object.to_serde_value();
    edit(&object)?;
    if object.to_serde_value() == original {
        return Ok(());
    }
    *after = FileSnapshot::new(root.to_string(), 0o600);
    Ok(())
}
