use std::{fs, io::Write as _, path::Path, sync::Mutex};

use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn read_text(path: &Path) -> Result<Option<String>, String> {
    log::debug!("reading TOML config: {}", path.display());
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

fn parse(text: &str) -> Result<DocumentMut, String> {
    // Omit source text because configuration files may contain credentials.
    text.parse::<DocumentMut>()
        .map_err(|error| format!("invalid TOML: {}", error.message()))
}

pub(crate) fn read(path: &Path) -> Result<DocumentMut, String> {
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
pub(crate) fn set_string(doc: &mut DocumentMut, keys: &[&str], text: &str) -> Result<(), String> {
    let (key, parents) = keys
        .split_last()
        .ok_or_else(|| "empty TOML key path".to_owned())?;
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
            .ok_or_else(|| format!("TOML field {parent} must be a table"))?;
    }
    let mut replacement = Value::from(text);
    if let Some(previous) = table.get(key).and_then(Item::as_value) {
        *replacement.decor_mut() = previous.decor().clone();
    }
    table.insert(key, Item::Value(replacement));
    Ok(())
}

/// Hides an empty parent section without removing its values or comments.
pub(crate) fn implicit_table(doc: &mut DocumentMut, keys: &[&str]) -> Result<(), String> {
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
pub(crate) fn inline_table(doc: &mut DocumentMut, keys: &[&str]) -> Result<(), String> {
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
        return Err("expected a TOML table".to_owned());
    }
    Ok(())
}

fn get_mut<'a>(doc: &'a mut DocumentMut, keys: &[&str]) -> Result<&'a mut Item, String> {
    let mut item = doc.as_item_mut();
    for key in keys {
        item = item
            .as_table_like_mut()
            .and_then(|table| table.get_mut(key))
            .ok_or_else(|| format!("TOML table not found: {key}"))?;
    }
    Ok(item)
}

pub(crate) fn remove(doc: &mut DocumentMut, keys: &[&str]) -> Result<(), String> {
    let (key, parents) = keys
        .split_last()
        .ok_or_else(|| "empty TOML key path".to_owned())?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for parent in parents {
        let Some(item) = table.get_mut(parent) else {
            return Ok(());
        };
        table = item
            .as_table_like_mut()
            .ok_or_else(|| format!("TOML field {parent} must be a table"))?;
    }
    table.remove(key);
    Ok(())
}

/// Applies one edit and atomically replaces the file. An absent, unchanged document is not created.
pub(crate) fn update(
    path: &Path,
    edit: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().map_err(|error| error.to_string())?;
    log::debug!("updating TOML config: {}", path.display());
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("config is a symlink; refusing to replace it".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
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
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    if original.is_some() {
        let permissions = fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|error| error.to_string())?;
    }
    temporary
        .write_all(updated.as_bytes())
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    if read_text(path)? != original {
        return Err("config changed during the operation; please retry".to_owned());
    }
    temporary
        .persist(path)
        .map_err(|error| format!("failed to save {}: {error}", path.display()))?;
    log::info!("TOML config saved: {}", path.display());
    Ok(())
}
