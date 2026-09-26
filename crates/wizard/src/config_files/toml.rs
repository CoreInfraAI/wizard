use std::{fs, io::Write as _, path::Path, sync::Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn read_text(path: &Path) -> Result<Option<String>> {
    log::debug!("reading TOML config: {}", path.display());
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn parse(text: &str) -> Result<DocumentMut> {
    // Omit source text because configuration files may contain credentials.
    text.parse::<DocumentMut>()
        .map_err(|_| anyhow!("invalid TOML"))
}

pub(crate) fn read(path: &Path) -> Result<DocumentMut> {
    parse(read_text(path)?.as_deref().unwrap_or_default())
}

pub(crate) fn get_string<'a>(doc: &'a DocumentMut, keys: &[&str]) -> Option<&'a str> {
    let mut item = doc.as_item();
    for key in keys {
        item = item.as_table_like()?.get(key)?;
    }
    item.as_str()
}

/// Updates a string value, preserving its comments and unrelated table entries.
pub(crate) fn set_string(doc: &mut DocumentMut, keys: &[&str], text: &str) -> Result<()> {
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
    let mut replacement = Value::from(text);
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

/// Formats a table inline. Existing commented sections retain their layout.
pub(crate) fn inline_table(doc: &mut DocumentMut, keys: &[&str]) -> Result<()> {
    let item = get_mut(doc, keys)?;
    if let Some(table) = item.as_table() {
        // Conversion formats away comments; retain the original table in that case.
        if table.to_string().contains('#')
            || table
                .decor()
                .prefix()
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.contains('#'))
            || table
                .decor()
                .suffix()
                .and_then(|s| s.as_str())
                .is_some_and(|s| s.contains('#'))
        {
            return Ok(());
        }
        let mut inline = table.clone().into_inline_table();
        inline.fmt();
        *item = Item::Value(Value::InlineTable(inline));
        if let Some((key, parents)) = keys.split_last()
            && let Some(mut key) = get_mut(doc, parents)?
                .as_table_like_mut()
                .and_then(|table| table.key_mut(key))
        {
            key.leaf_decor_mut().set_suffix(" ");
        }
    } else if !item.is_inline_table() {
        bail!("expected a TOML table");
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

/// Applies one edit and atomically replaces the file. An absent, unchanged document is not created.
pub(crate) fn update(path: &Path, edit: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .map_err(|_| anyhow!("TOML write lock poisoned"))?;
    log::debug!("updating TOML config: {}", path.display());
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("config is a symlink; refusing to replace it");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect config file"),
    }
    let original = read_text(path)?;
    let mut doc = parse(original.as_deref().unwrap_or_default())?;
    edit(&mut doc)?;
    let updated = doc.to_string();
    if original.as_deref().unwrap_or_default() == updated {
        log::info!(
            "TOML config already matches requested changes: {}",
            path.display()
        );
        return Ok(());
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).context("failed to create config directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create temporary config file")?;
    if original.is_some() {
        let permissions = fs::metadata(path)
            .context("failed to read config permissions")?
            .permissions();
        temporary
            .as_file()
            .set_permissions(permissions)
            .context("failed to preserve config permissions")?;
    }
    temporary
        .write_all(updated.as_bytes())
        .context("failed to write config")?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync config")?;
    if read_text(path)? != original {
        bail!("config changed during the operation; please retry");
    }
    temporary
        .persist(path)
        .with_context(|| format!("failed to save {}", path.display()))?;
    log::info!("TOML config saved: {}", path.display());
    Ok(())
}
