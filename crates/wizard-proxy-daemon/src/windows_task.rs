use std::io;

// Kept platform-independent so XML escaping is tested without registering tasks.
pub(super) fn definition(sid: &str, executable: &str) -> io::Result<String> {
    Ok(format!(
        include_str!("windows-task.xml"),
        sid = xml_text(sid)?,
        executable = xml_text(executable)?,
    ))
}

pub(super) fn sid_from_whoami(output: &[u8]) -> io::Result<String> {
    // whoami /user /fo csv /nh ends its row with a quoted ASCII SID. Ignore
    // the account name: it may contain commas, escaped quotes or non-UTF-8 bytes.
    let output = String::from_utf8_lossy(output);
    let sid = output
        .trim()
        .rsplit_once(',')
        .and_then(|(_, value)| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
        .filter(|sid| {
            sid.strip_prefix("S-1-").is_some_and(|rest| {
                rest.split('-')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            })
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid current user SID response",
            )
        })?;
    Ok(sid.to_owned())
}

fn xml_text(value: &str) -> io::Result<String> {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t'
            | '\n'
            | '\r'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => escaped.push(character),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid XML character in proxy task definition",
                ));
            }
        }
    }
    Ok(escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whoami_sid_does_not_depend_on_account_name_encoding_or_csv_escaping() {
        let output = b"\"domain\\na\xffme, \"\"quoted\"\"\",\"S-1-5-21-123-456-789-1001\"\r\n";
        assert_eq!(
            sid_from_whoami(output).expect("SID"),
            "S-1-5-21-123-456-789-1001"
        );
        for invalid in [
            &b""[..],
            &b"\"name\",\"not-a-sid\""[..],
            &b"\"name\",\"S-1-5-\""[..],
            &b"\"name\",\"S-1-5\"extra"[..],
        ] {
            assert!(sid_from_whoami(invalid).is_err());
        }
    }

    #[test]
    fn executable_is_xml_text_not_markup() {
        let xml = definition("S-1-5-21-123", r#"C:\工具 & <dev>\Wizard "test'.exe"#)
            .expect("task definition");
        assert!(
            xml.contains(
                r"<Command>C:\工具 &amp; &lt;dev&gt;\Wizard &quot;test&apos;.exe</Command>",
            )
        );
        let injection =
            definition("S-1-5-21-123", "</Command><Arguments>injected").expect("escaped markup");
        assert_eq!(injection.matches("<Arguments>").count(), 1);
        assert!(definition("S-1-5-21-123", "invalid\0path").is_err());
    }
}
