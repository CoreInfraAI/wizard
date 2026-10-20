use super::changes::FileSnapshot;
use anyhow::{Context as _, Result, ensure};

/// Replaces variables in the supplied order, preserving unrelated records verbatim.
/// Entries are `(key, value)`. Dollar signs are preserved for dotenv interpolation.
/// Changes only the working snapshot. Never include parser errors or values in diagnostics.
pub(crate) fn set_many(after: &mut FileSnapshot, entries: &[(&str, Option<&str>)]) -> Result<()> {
    let original_content = after.content();
    let mut updated = original_content.unwrap_or_default().to_owned();
    let newline = if updated.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    // Remove first so references are written after the variables they depend on.
    for (key, _) in entries {
        updated = replace(&updated, key, None, newline)?;
    }
    for (key, value) in entries {
        updated = replace(&updated, key, *value, newline)?;
    }
    if original_content.unwrap_or_default() == updated {
        return Ok(());
    }
    *after = FileSnapshot::new(updated, 0o600);
    Ok(())
}

fn replace(text: &str, key: &str, value: Option<&str>, newline: &str) -> Result<String> {
    ensure!(valid_key(key), "invalid env variable name");
    let replacement = value
        .map(|value| {
            ensure!(
                !value.contains(['\0', '\r']),
                "env value must not contain NUL or carriage return"
            );
            let escaped = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
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

/// Returns the first assignment of `key`, unquoted but without `${...}` interpolation.
/// Never include parser errors or values in diagnostics.
pub(crate) fn get(snapshot: &FileSnapshot, key: &str) -> Result<Option<String>> {
    for line in snapshot.content().unwrap_or_default().split_inclusive('\n') {
        if let Some((line_key, value)) = parse_line(line)?
            && line_key == key
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn line_key(line: &str) -> Result<Option<&str>> {
    Ok(parse_line(line)?.map(|(key, _)| key))
}

/// Recognizes single-line assignments and decodes quotes without evaluating interpolation.
/// Unsupported/malformed records fail closed; errors never include file contents.
fn parse_line(line: &str) -> Result<Option<(&str, String)>> {
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
        return Ok(Some((key, String::new())));
    }
    let mut decoded = String::new();
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
            decoded.push(if quote == Some('"') && ch == 'n' {
                '\n'
            } else {
                ch
            });
        } else if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else {
                decoded.push(ch);
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
            } else {
                decoded.push(ch);
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            trailing = true;
        } else {
            decoded.push(ch);
        }
    }
    ensure!(
        quote.is_none() && !escaped,
        "unterminated env value; multiline values are not supported"
    );
    Ok(Some((key, decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_decodes_written_values() {
        let mut snapshot = FileSnapshot::Missing;
        set_many(&mut snapshot, &[("KEY", Some("a \"b\" \\ ${C}"))]).expect("fixture value");
        assert_eq!(
            get(&snapshot, "KEY").expect("fixture env").as_deref(),
            Some("a \"b\" \\ ${C}")
        );

        let snapshot = FileSnapshot::new(
            "# KEY=comment\nexport KEY='x y' # note\nKEY=second\nOTHER=plain\n".into(),
            0o600,
        );
        assert_eq!(
            get(&snapshot, "KEY").expect("fixture env").as_deref(),
            Some("x y")
        );
        assert_eq!(
            get(&snapshot, "OTHER").expect("fixture env").as_deref(),
            Some("plain")
        );
        assert_eq!(get(&snapshot, "MISSING").expect("fixture env"), None);
    }
}
