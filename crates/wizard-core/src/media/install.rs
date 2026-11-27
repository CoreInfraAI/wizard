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

const SERVER: &str = "coreinfra_media";
/// The 1.0.10 registration, removed wherever Wizard owns it.
pub(crate) const LEGACY_SERVER: &str = "coreinfra_video";
/// Skill documents bundled for one agent: directory name, then relative paths and contents.
pub(crate) type Skills = [(&'static str, &'static [(&'static str, &'static str)])];
const SKILLS: &Skills = &[
    (
        "coreinfra-video",
        &[
            (
                "SKILL.md",
                include_str!("../../../../skills/coreinfra-video/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../../../../skills/coreinfra-video/agents/openai.yaml"),
            ),
        ],
    ),
    (
        "coreinfra-speech",
        &[
            (
                "SKILL.md",
                include_str!("../../../../skills/coreinfra-speech/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../../../../skills/coreinfra-speech/agents/openai.yaml"),
            ),
        ],
    ),
    (
        "coreinfra-transcription",
        &[
            (
                "SKILL.md",
                include_str!("../../../../skills/coreinfra-transcription/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../../../../skills/coreinfra-transcription/agents/openai.yaml"),
            ),
        ],
    ),
    (
        "coreinfra-image",
        &[
            (
                "SKILL.md",
                include_str!("../../../../skills/coreinfra-image/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../../../../skills/coreinfra-image/agents/openai.yaml"),
            ),
        ],
    ),
];

pub fn sync(
    agent: wizard_settings::MediaAgent,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<()> {
    use wizard_settings::MediaAgent::{Claude, ClaudeDesktop, Codex};
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
                claude::sync_media(false)
            } else {
                // Register first, so a failed registration leaves no usable bearer behind.
                claude::sync_media(true)?;
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
        "media-claude-desktop-headers.json"
    } else {
        "media-claude-code-headers.json"
    };
    Ok(wizard_settings::paths::config_dir()?.join(name))
}

/// Where 1.0.10 kept the same grant, registered by its `coreinfra_video` entries.
pub(crate) fn legacy_headers(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?.strip_prefix("media-")?;
    Some(path.with_file_name(format!("video-{name}")))
}

/// Move a 1.0.10 headers file to `path` in the plan that rewrites its registration, so the
/// agent keeps its grant even if no media change follows. Without `keep` only remove it.
pub(crate) fn migrate_headers(path: &Path, keep: bool) -> Result<Vec<FileChange>> {
    let Some(legacy) = legacy_headers(path) else {
        return Ok(Vec::new());
    };
    let mut old = FileChange::read(legacy)?;
    let Some(content) = old.before().content().map(str::to_owned) else {
        return Ok(Vec::new());
    };
    old.after = FileSnapshot::Missing;
    let mut current = FileChange::read(path.to_owned())?;
    if keep && current.before().content().is_none() {
        current.after = FileSnapshot::new(content, 0o600);
    }
    Ok(vec![current, old])
}

pub(crate) fn sync_headers(path: &Path, bearer: Option<&str>) -> Result<()> {
    let mut grant = FileChange::read(path.to_owned())?;
    grant.after = bearer.map_or(FileSnapshot::Missing, |token| {
        FileSnapshot::new(
            serde_json::json!({"Authorization": format!("Bearer {token}")}).to_string(),
            0o600,
        )
    });
    let mut changes = vec![grant];
    if let Some(legacy) = legacy_headers(path) {
        let mut legacy = FileChange::read(legacy)?;
        legacy.after = FileSnapshot::Missing;
        changes.push(legacy);
    }
    FileChange::apply_all(&changes)
}

pub(super) fn sync_at(path: &Path, previous: Option<&str>, next: Option<&str>) -> Result<()> {
    let (config, skills) = changes_at(path, previous, next)?;
    if next.is_none() {
        // Revoke both clients even if a user-edited skill cannot be cleaned up.
        FileChange::apply_all(&[config])?;
        return FileChange::apply_all(&skills?);
    }
    let mut changes = skills?;
    changes.push(config);
    FileChange::apply_all(&changes)
}

/// The Codex configuration change, then the skill changes, without writing files.
fn changes_at(
    path: &Path,
    previous: Option<&str>,
    next: Option<&str>,
) -> Result<(FileChange, Result<Vec<FileChange>>)> {
    let mut config = FileChange::read(path.to_owned())?;
    update_config(&mut config.after, previous, next)?;
    let home = path.parent().context("Missing Codex home")?;
    Ok((
        config,
        skills_changes(&home.join("skills"), SKILLS, next.is_some()),
    ))
}

/// Whether applying the saved `grant` would change the Codex profile at `path`,
/// such as a 1.0.10 registration or missing audio skills after an upgrade.
pub(crate) fn needs_apply_at(path: &Path, grant: Option<&str>) -> bool {
    changes_at(path, grant, grant).map_or(true, |(config, skills)| {
        config.is_changed()
            || skills.map_or(true, |skills| skills.iter().any(FileChange::is_changed))
    })
}

/// Install or remove each bundled skill under `root`. A disabled skill without a
/// Wizard manifest was never installed and is left alone.
pub(crate) fn skills_changes(
    root: &Path,
    skills: &Skills,
    enable: bool,
) -> Result<Vec<FileChange>> {
    let mut changes = Vec::new();
    for &(name, files) in skills {
        let dir = root.join(name);
        if !enable && !dir.join(".wizard-managed.json").exists() {
            continue;
        }
        changes.extend(skill_changes(&dir, files, enable)?);
    }
    Ok(changes)
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
                "Media skill directory is a symlink: {}",
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

/// A Wizard bearer: the saved grant, or one issued by this or a 1.0.10 version.
fn wizard_bearer(value: Option<&str>, previous: Option<&str>, next: Option<&str>) -> bool {
    value
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|bearer| {
            previous == Some(bearer)
                || next == Some(bearer)
                || ["wizard-media-", "wizard-video-"].iter().any(|prefix| {
                    bearer
                        .strip_prefix(prefix)
                        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                })
        })
}

fn owned(doc: &DocumentMut, name: &str, previous: Option<&str>, next: Option<&str>) -> bool {
    toml::get_string(doc, &["mcp_servers", name, "url"]) == Some(super::URL)
        && wizard_bearer(
            toml::get_string(doc, &["mcp_servers", name, "http_headers", "Authorization"]),
            previous,
            next,
        )
}

fn registration(doc: &mut DocumentMut, previous: Option<&str>, next: Option<&str>) -> Result<()> {
    let server_exists = doc.get("mcp_servers").and_then(|i| i.get(SERVER)).is_some();
    let server_owned = server_exists && owned(doc, SERVER, previous, next);
    let legacy_owned = owned(doc, LEGACY_SERVER, previous, next);
    if server_exists && !server_owned {
        ensure!(
            next.is_none(),
            "Имя MCP-сервера coreinfra_media уже занято в config.toml"
        );
        // Preserve the foreign server while still cleaning up an owned 1.0.10 entry.
        if legacy_owned {
            toml::remove(doc, &["mcp_servers", LEGACY_SERVER])?;
        }
        return Ok(());
    }
    if let Some(bearer) = next {
        if legacy_owned {
            if !server_exists
                && let Some(servers) = doc
                    .get_mut("mcp_servers")
                    .and_then(toml_edit::Item::as_table_like_mut)
                && let Some(legacy) = servers.remove(LEGACY_SERVER)
            {
                // Move the 1.0.10 table intact so user restrictions (enabled, tool filters, timeout) survive.
                servers.insert(SERVER, legacy);
            } else {
                toml::remove(doc, &["mcp_servers", LEGACY_SERVER])?;
            }
        }
        let has_timeout = doc
            .get("mcp_servers")
            .and_then(|i| i.get(SERVER))
            .and_then(|s| s.get("tool_timeout_sec"))
            .is_some();
        toml::set_value(doc, &["mcp_servers", SERVER, "url"], super::URL)?;
        if !has_timeout {
            toml::set_value(doc, &["mcp_servers", SERVER, "tool_timeout_sec"], 240)?;
        }
        toml::set_value(
            doc,
            &["mcp_servers", SERVER, "http_headers", "Authorization"],
            format!("Bearer {bearer}"),
        )
    } else {
        if legacy_owned {
            toml::remove(doc, &["mcp_servers", LEGACY_SERVER])?;
        }
        if server_owned {
            toml::remove(doc, &["mcp_servers", SERVER])?;
        }
        Ok(())
    }
}
