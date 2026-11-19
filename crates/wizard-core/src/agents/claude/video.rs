//! User-scoped skill and MCP registration for Claude Code gateway profiles.
use crate::{
    config_files::{changes::FileChange, json},
    video::install,
};
use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

const SERVER: &str = "coreinfra_video";
const FILES: &[(&str, &str)] = &[(
    "SKILL.md",
    include_str!("../../../../../plugins/coreinfra-video/skills/coreinfra-video/SKILL.md"),
)];

pub(super) fn server(url: &str, headers: &Path) -> Result<Value> {
    let path = headers
        .to_str()
        .context("Video headers path must be Unicode")?;
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
    Ok(json!({"type":"http", "url":url, "headersHelper":helper}))
}

pub(super) fn configure(
    dir: &Path,
    global: &mut FileChange,
    server: &Value,
    enable: bool,
) -> Result<Vec<FileChange>> {
    json::update(&mut global.after, |config| {
        let existing = config["mcpServers"].get(SERVER);
        let owned = existing.is_some_and(|entry| {
            ["type", "url", "headersHelper"]
                .iter()
                .all(|&key| entry[key] == server[key])
        });
        if enable {
            ensure!(
                owned || existing.is_none(),
                "В Claude Code уже зарегистрирован другой MCP-сервер coreinfra_video"
            );
            if !owned {
                config
                    .as_object_mut()
                    .context("Invalid Claude Code config")?
                    .entry("mcpServers")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .context("Claude Code mcpServers must be an object")?
                    .insert(SERVER.into(), server.clone());
            }
        } else if owned && let Some(servers) = config["mcpServers"].as_object_mut() {
            servers.shift_remove(SERVER);
        }
        Ok(())
    })?;
    let root = dir.join("skills/coreinfra-video");
    if !enable && !root.join(".wizard-managed.json").exists() {
        return Ok(Vec::new());
    }
    install::skill_changes(&root, FILES, enable)
}
