use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

use super::changes::FileSnapshot;

fn parse(text: &str) -> Result<DocumentMut> {
    // Omit source text because configuration files may contain credentials.
    text.parse::<DocumentMut>()
        .map_err(|_| anyhow!("invalid TOML"))
}

pub(crate) fn read(path: &Path) -> Result<DocumentMut> {
    log::debug!("reading TOML config: {}", path.display());
    let snapshot =
        FileSnapshot::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse(snapshot.as_ref().map_or("", |snapshot| &snapshot.content))
}

pub(crate) fn get_string<'a>(doc: &'a DocumentMut, keys: &[&str]) -> Option<&'a str> {
    let mut item = doc.as_item();
    for key in keys {
        item = item.as_table_like()?.get(key)?;
    }
    item.as_str()
}

#[expect(dead_code)]
pub(crate) fn get_value<'a>(doc: &'a DocumentMut, keys: &[&str]) -> Option<&'a Value> {
    let mut item = doc.as_item();
    for key in keys {
        item = item.as_table_like()?.get(key)?;
    }
    item.as_value()
}

/// Updates a value, preserving its comments and unrelated table entries.
pub(crate) fn set_value(
    doc: &mut DocumentMut,
    keys: &[&str],
    value: impl Into<Value>,
) -> Result<()> {
    let (key, parents) = keys.split_last().context("empty TOML key path")?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for parent in parents {
        if !table.contains_key(parent) {
            let mut new_table = Table::new();
            new_table.set_implicit(true);
            table.insert(parent, Item::Table(new_table));
        }
        table = table
            .get_mut(parent)
            .and_then(Item::as_table_like_mut)
            .with_context(|| format!("TOML field {parent} must be a table"))?;
    }
    let mut replacement = value.into();
    if let Some(previous) = table.get(key).and_then(Item::as_value) {
        *replacement.decor_mut() = previous.decor().clone();
    }
    table.insert(key, Item::Value(replacement));
    Ok(())
}

/// Hides an empty parent section without removing its values or comments.
pub(crate) fn implicit_table(doc: &mut DocumentMut, keys: &[&str]) -> Result<()> {
    let item = get_mut(doc, keys)?;
    if let Some(table) = item.as_table_mut() {
        // Hiding the header would also hide comments attached to it.
        if !table
            .decor()
            .prefix()
            .and_then(|s| s.as_str())
            .is_some_and(|s| s.contains('#'))
            && !table
                .decor()
                .suffix()
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.contains('#'))
        {
            table.set_implicit(true);
        }
    }
    Ok(())
}

fn get_mut<'a>(doc: &'a mut DocumentMut, keys: &[&str]) -> Result<&'a mut Item> {
    let mut item = doc.as_item_mut();
    for key in keys {
        item = item
            .as_table_like_mut()
            .and_then(|table| table.get_mut(key))
            .with_context(|| format!("TOML table not found: {key}"))?;
    }
    Ok(item)
}

pub(crate) fn remove(doc: &mut DocumentMut, keys: &[&str]) -> Result<()> {
    let (key, parents) = keys.split_last().context("empty TOML key path")?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for parent in parents {
        let Some(item) = table.get_mut(parent) else {
            return Ok(());
        };
        table = item
            .as_table_like_mut()
            .with_context(|| format!("TOML field {parent} must be a table"))?;
    }
    table.remove(key);
    Ok(())
}

/// Edits only the working snapshot, preserving comments and existing permissions.
/// An absent, unchanged document remains absent.
pub(crate) fn update(
    after: &mut Option<FileSnapshot>,
    edit: impl FnOnce(&mut DocumentMut) -> Result<()>,
) -> Result<()> {
    let original_content = after
        .as_ref()
        .map_or("", |snapshot| snapshot.content.as_str());
    let mut doc = parse(original_content)?;
    edit(&mut doc)?;
    let updated = doc.to_string();
    if original_content == updated {
        return Ok(());
    }
    match after {
        Some(snapshot) => snapshot.content = updated,
        None => *after = Some(FileSnapshot::new(updated, 0o600)),
    }
    Ok(())
}
