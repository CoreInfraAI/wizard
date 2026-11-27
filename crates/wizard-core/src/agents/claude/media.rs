//! User-scoped skills and MCP registration for Claude Code gateway profiles.
use crate::{
    config_files::{changes::FileChange, json},
    media::install::{self, LEGACY_SERVER, Skills},
};
use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SERVER: &str = "coreinfra_media";
const SKILLS: &Skills = &[
    (
        "coreinfra-video",
        &[(
            "SKILL.md",
            include_str!("../../../../../plugins/coreinfra-media/skills/coreinfra-video/SKILL.md"),
        )],
    ),
    (
        "coreinfra-speech",
        &[(
            "SKILL.md",
            include_str!("../../../../../plugins/coreinfra-media/skills/coreinfra-speech/SKILL.md"),
        )],
    ),
    (
        "coreinfra-transcription",
        &[(
            "SKILL.md",
            include_str!(
                "../../../../../plugins/coreinfra-media/skills/coreinfra-transcription/SKILL.md"
            ),
        )],
    ),
    (
        "coreinfra-image",
        &[(
            "SKILL.md",
            include_str!("../../../../../plugins/coreinfra-media/skills/coreinfra-image/SKILL.md"),
        )],
    ),
];

/// The registration Wizard writes, and the helper of the 1.0.10 one it replaces.
#[cfg_attr(test, derive(Debug))]
pub(super) struct Server {
    pub(super) entry: Value,
    headers: PathBuf,
    legacy_helper: Option<String>,
}

fn headers_helper(headers: &Path) -> Result<String> {
    let path = headers
        .to_str()
        .context("Media headers path must be Unicode")?;
    // No runtime dependencies. The path is data, including spaces and shell metacharacters.
    #[cfg(unix)]
    let helper = format!("/bin/cat '{}'", path.replace('\'', "'\\''"));
    #[cfg(windows)]
    let helper = {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let script = format!(
            "[Console]::Write([IO.File]::ReadAllText('{}'))",
            path.replace('\'', "''")
        );
        let encoded = STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        format!("powershell.exe -NoProfile -NonInteractive -EncodedCommand {encoded}")
    };
    Ok(helper)
}

pub(super) fn server(url: &str, headers: &Path) -> Result<Server> {
    let legacy_helper = install::legacy_headers(headers)
        .as_deref()
        .map(headers_helper)
        .transpose()?;
    Ok(Server {
        entry: json!({"type":"http", "url":url, "headersHelper":headers_helper(headers)?, "timeout":240_000}),
        headers: headers.to_owned(),
        legacy_helper,
    })
}

pub(super) fn configure(
    dir: &Path,
    global: &mut FileChange,
    server: &Server,
    enable: bool,
) -> Result<Vec<FileChange>> {
    let entry = &server.entry;
    json::update(&mut global.after, |config| {
        let existing = config["mcpServers"].get(SERVER);
        let exists = existing.is_some();
        let owned = existing.is_some_and(|current| {
            ["type", "url", "headersHelper"]
                .iter()
                .all(|&key| current[key] == entry[key])
        });
        // Wizard 1.0.10 registered the same endpoint under the old name and headers file.
        let legacy = config["mcpServers"]
            .get(LEGACY_SERVER)
            .is_some_and(|current| {
                current["type"] == entry["type"]
                    && current["url"] == entry["url"]
                    && server
                        .legacy_helper
                        .as_deref()
                        .is_some_and(|helper| current["headersHelper"] == helper)
            });
        if legacy && let Some(servers) = config["mcpServers"].as_object_mut() {
            servers.shift_remove(LEGACY_SERVER);
        }
        if enable {
            ensure!(
                owned || !exists,
                "В Claude Code уже зарегистрирован другой MCP-сервер coreinfra_media"
            );
            let servers = config
                .as_object_mut()
                .context("Invalid Claude Code config")?
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("Claude Code mcpServers must be an object")?;
            if !owned {
                servers.insert(SERVER.into(), entry.clone());
            } else if servers[SERVER].get("timeout").is_none_or(Value::is_null) {
                // An older Wizard wrote no timeout. A value that is there is the user's and stays.
                servers[SERVER]["timeout"] = entry["timeout"].clone();
            }
        } else if owned && let Some(servers) = config["mcpServers"].as_object_mut() {
            servers.shift_remove(SERVER);
        }
        Ok(())
    })?;
    // Keep the grant reachable for the rewritten registration until the grant itself changes.
    let mut changes = install::migrate_headers(&server.headers, enable)?;
    changes.extend(install::skills_changes(
        &dir.join("skills"),
        SKILLS,
        enable,
    )?);
    Ok(changes)
}
