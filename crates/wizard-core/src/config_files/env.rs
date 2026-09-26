use std::{fs, io::Write as _, path::Path};

use anyhow::{Context as _, Result, bail, ensure};

/// Replaces one variable, preserving unrelated dotenv records verbatim.
/// Callers must serialize writes. Never include parser errors or values in diagnostics.
pub(crate) fn set(path: &Path, key: &str, value: Option<&str>) -> Result<()> {
    let original = read(path)?;
    let updated = replace(original.as_deref().unwrap_or_default(), key, value)?;
    if original.as_deref().unwrap_or_default() == updated {
        return Ok(());
    }
    let parent = path.parent().context("env file path has no parent")?;
    fs::create_dir_all(parent).context("failed to create env file directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("failed to create temporary env file")?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to restrict env file permissions")?;
    }
    temporary
        .write_all(updated.as_bytes())
        .context("failed to write env file")?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync env file")?;
    ensure!(
        read(path)? == original,
        "env file changed during the operation; please retry"
    );
    temporary.persist(path).context("failed to save env file")?;
    Ok(())
}

fn read(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("env file is a symlink; refusing to replace it");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to inspect env file"),
    }
    fs::read_to_string(path)
        .map(Some)
        .context("failed to read env file")
}

fn replace(text: &str, key: &str, value: Option<&str>) -> Result<String> {
    ensure!(valid_key(key), "invalid env variable name");
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let replacement = value
        .map(|value| {
            ensure!(
                !value.contains(['\0', '\r']),
                "env value must not contain NUL or carriage return"
            );
            let escaped = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
                .replace('\n', "\\n");
            Ok(format!("{key}=\"{escaped}\"{newline}"))
        })
        .transpose()?;
    let mut output = String::new();
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        if line_key(line)? == Some(key) {
            if !replaced && let Some(replacement) = &replacement {
                output.push_str(replacement);
            }
            replaced = true;
        } else {
            output.push_str(line);
        }
    }
    if !replaced && let Some(replacement) = replacement {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push_str(newline);
        }
        output.push_str(&replacement);
    }
    Ok(output)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().enumerate().all(|(index, ch)| {
            ch == b'_' || ch.is_ascii_alphabetic() || (index > 0 && ch.is_ascii_digit())
        })
}

/// Recognizes single-line assignments without evaluating or decoding their values.
/// Unsupported/malformed records fail closed; errors never include file contents.
fn line_key(line: &str) -> Result<Option<&str>> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line).trim_start();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let line = line
        .strip_prefix("export ")
        .or_else(|| line.strip_prefix("export\t"))
        .unwrap_or(line)
        .trim_start();
    let (key, value) = line.split_once('=').context("expected an env assignment")?;
    let key = key.trim_end();
    ensure!(valid_key(key), "invalid env variable name");
    let value = value.trim_start();
    if value.starts_with('#') {
        return Ok(Some(key));
    }
    let mut quote = None;
    let mut escaped = false;
    let mut trailing = false;
    for ch in value.chars() {
        ensure!(
            !matches!(ch, '\0' | '\r' | '\n'),
            "unsupported control character in env value"
        );
        if escaped {
            escaped = false;
        } else if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            }
        } else if trailing {
            if ch == '#' {
                break;
            }
            ensure!(ch.is_whitespace(), "unexpected text after env value");
        } else if ch == '\\' {
            escaped = true;
        } else if quote == Some('"') {
            if ch == '"' {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            trailing = true;
        }
    }
    ensure!(
        quote.is_none() && !escaped,
        "unterminated env value; multiline values are not supported"
    );
    Ok(Some(key))
}
