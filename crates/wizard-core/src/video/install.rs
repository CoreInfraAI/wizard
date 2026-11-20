//! Reconcile only the Wizard-owned MCP registration and bundled skill documents.
use crate::{
    agents::{claude, codex},
    config_files::{
        changes::{FileChange, FileSnapshot},
        toml,
    },
    prepared_plan::PreparedAgent,
};
use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, ensure};
use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;

const SERVER: &str = "coreinfra_video";
const SKILL: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../../skills/coreinfra-video/SKILL.md"),
    ),
    (
        "agents/openai.yaml",
        include_str!("../../../../skills/coreinfra-video/agents/openai.yaml"),
    ),
];

pub fn sync(
    agent: wizard_settings::VideoAgent,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<()> {
    use wizard_settings::VideoAgent::{Claude, ClaudeDesktop, Codex};
    match agent {
        Codex => {
            let _guard = PreparedAgent::Codex.lock()?;
            sync_at(&codex::config_path()?, previous, next)
        }
        Claude => {
            let headers = headers_path(false)?;
            if next.is_none() {
                // Revoke the grant first: a user-edited skill may block removing the registration.
                sync_headers(&headers, None)?;
                claude::sync_video(false)
            } else {
                // Register first, so a failed registration leaves no usable bearer behind.
                claude::sync_video(true)?;
                sync_headers(&headers, next)
            }
        }
        // The routing plan applied just before writes Claude Desktop's registration,
        // since the app must be closed for it.
        ClaudeDesktop => sync_headers(&headers_path(true)?, next),
    }
}

pub(crate) fn headers_path(desktop: bool) -> Result<PathBuf> {
    let name = if desktop {
        "video-claude-desktop-headers.json"
    } else {
        "video-claude-code-headers.json"
    };
    Ok(wizard_settings::paths::config_dir()?.join(name))
}

pub(crate) fn sync_headers(path: &Path, bearer: Option<&str>) -> Result<()> {
    let mut grant = FileChange::read(path.to_owned())?;
    grant.after = bearer.map_or(FileSnapshot::Missing, |token| {
        FileSnapshot::new(
            serde_json::json!({"Authorization": format!("Bearer {token}")}).to_string(),
            0o600,
        )
    });
    FileChange::apply_all(&[grant])
}

pub(super) fn sync_at(path: &Path, previous: Option<&str>, next: Option<&str>) -> Result<()> {
    let mut config = FileChange::read(path.to_owned())?;
    update_config(&mut config.after, previous, next)?;
    let root = path
        .parent()
        .context("Missing Codex home")?
        .join("skills/coreinfra-video");
    if next.is_none() {
        // Revoke both clients even if a user-edited skill cannot be cleaned up.
        FileChange::apply_all(&[config])?;
        return FileChange::apply_all(&skill_changes(&root, SKILL, false)?);
    }
    let mut changes = skill_changes(&root, SKILL, true)?;
    changes.push(config);
    FileChange::apply_all(&changes)
}

pub(crate) fn skill_changes(
    root: &Path,
    files: &[(&str, &str)],
    enable: bool,
) -> Result<Vec<FileChange>> {
    for &(name, _) in files {
        let path = root.join(name);
        for dir in path
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(root))
        {
            ensure!(
                !dir.is_symlink(),
                "Video skill directory is a symlink: {}",
                dir.display()
            );
        }
    }
    let mut manifest = FileChange::read(root.join(".wizard-managed.json"))?;
    let old: BTreeMap<String, String> = manifest
        .before()
        .content()
        .map(serde_json::from_str)
        .transpose()
        .context("Invalid Wizard skill manifest")?
        .unwrap_or_default();
    let mut new = BTreeMap::new();
    let mut changes = Vec::new();
    for &(name, text) in files {
        let mut file = FileChange::read(root.join(name))?;
        let owned = file
            .before()
            .content()
            .is_some_and(|v| old.get(name) == Some(&super::fingerprint(v)));
        if enable {
            ensure!(
                owned
                    || file.before().content().is_none()
                    || (old.contains_key(name) && file.before().content() == Some(text)),
                "Файл скилла изменён пользователем: {}",
                file.path().display()
            );
            file.after = FileSnapshot::new(text.into(), 0o644);
            new.insert(name, super::fingerprint(text));
        } else if owned {
            file.after = FileSnapshot::Missing;
        }
        changes.push(file);
    }
    manifest.after = if enable {
        FileSnapshot::new(serde_json::to_string_pretty(&new)?, 0o600)
    } else {
        FileSnapshot::Missing
    };
    if enable && manifest.before().content().is_none() {
        // A retry can recognize documents written before a first install was interrupted.
        changes.insert(0, manifest);
    } else {
        changes.push(manifest);
    }
    Ok(changes)
}

pub(crate) fn update_config(
    snapshot: &mut FileSnapshot,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<()> {
    toml::update(snapshot, |doc| registration(doc, previous, next))?;
    #[cfg(unix)]
    if next.is_some()
        && let FileSnapshot::Present { mode, .. } = snapshot
    {
        *mode &= 0o600;
    }
    Ok(())
}

fn registration(doc: &mut DocumentMut, previous: Option<&str>, next: Option<&str>) -> Result<()> {
    let key = ["mcp_servers", SERVER];
    let table = doc.get("mcp_servers").and_then(|i| i.get(SERVER));
    let needs_timeout = table.and_then(|s| s.get("tool_timeout_sec")).is_none();
    if table.is_some() {
        let url = toml::get_string(doc, &["mcp_servers", SERVER, "url"]);
        let auth = toml::get_string(
            doc,
            &["mcp_servers", SERVER, "http_headers", "Authorization"],
        );
        let ours = url == Some(super::URL)
            && auth.is_some_and(|value| {
                value.strip_prefix("Bearer ").is_some_and(|bearer| {
                    previous == Some(bearer)
                        || next == Some(bearer)
                        || bearer
                            .strip_prefix("wizard-video-")
                            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                })
            });
        if !ours {
            ensure!(
                next.is_none(),
                "Имя MCP-сервера coreinfra_video уже занято в config.toml"
            );
            return Ok(());
        }
    }
    if let Some(bearer) = next {
        toml::set_value(doc, &["mcp_servers", SERVER, "url"], super::URL)?;
        if needs_timeout {
            toml::set_value(doc, &["mcp_servers", SERVER, "tool_timeout_sec"], 240)?;
        }
        toml::set_value(
            doc,
            &["mcp_servers", SERVER, "http_headers", "Authorization"],
            format!("Bearer {bearer}"),
        )
    } else {
        toml::remove(doc, &key)
    }
}
