//! Reconcile only the Wizard-owned MCP registration and bundled skill documents.
use crate::{
    agents::codex,
    config_files::{
        changes::{FileChange, FileSnapshot},
        toml,
    },
    prepared_plan::PreparedAgent,
};
use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, ensure};
use std::path::Path;
use toml_edit::DocumentMut;
use wizard_settings::VideoConnection;

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

pub fn sync(previous: Option<&VideoConnection>, next: Option<&VideoConnection>) -> Result<()> {
    let _guard = PreparedAgent::Codex.lock()?;
    sync_at(&codex::config_path()?, previous, next)
}

pub(super) fn sync_at(
    path: &Path,
    previous: Option<&VideoConnection>,
    next: Option<&VideoConnection>,
) -> Result<()> {
    let mut config = FileChange::read(path.to_owned())?;
    update_config(&mut config.after, previous, next)?;
    if next.is_none() {
        // Revoke the registration even when a user-edited skill cannot be cleaned up.
        FileChange::apply_all(core::slice::from_ref(&config))?;
    }
    let root = path
        .parent()
        .context("Missing Codex home")?
        .join("skills/coreinfra-video");
    for dir in [&root, &root.join("agents")] {
        ensure!(
            !dir.is_symlink(),
            "Video skill directory is a symlink: {}",
            dir.display()
        );
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
    for &(name, text) in SKILL {
        let mut file = FileChange::read(root.join(name))?;
        let owned = file
            .before()
            .content()
            .is_some_and(|v| old.get(name) == Some(&super::fingerprint(v)));
        if next.is_some() {
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
    manifest.after = if next.is_some() {
        FileSnapshot::new(serde_json::to_string_pretty(&new)?, 0o600)
    } else {
        FileSnapshot::Missing
    };
    if next.is_some() && manifest.before().content().is_none() {
        // A retry can recognize documents written before a first install was interrupted.
        changes.insert(0, manifest);
    } else {
        changes.push(manifest);
    }
    if next.is_some() {
        changes.push(config);
    }
    FileChange::apply_all(&changes)
}

pub(crate) fn update_config(
    snapshot: &mut FileSnapshot,
    previous: Option<&VideoConnection>,
    next: Option<&VideoConnection>,
) -> Result<()> {
    toml::update(snapshot, |doc| {
        registration(doc, previous, next)?;
        if next.is_some() {
            codex::exclude_hub_key(doc)?;
        }
        Ok(())
    })?;
    #[cfg(unix)]
    if next.is_some()
        && let FileSnapshot::Present { mode, .. } = snapshot
    {
        *mode &= 0o600;
    }
    Ok(())
}

fn registration(
    doc: &mut DocumentMut,
    previous: Option<&VideoConnection>,
    next: Option<&VideoConnection>,
) -> Result<()> {
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
                    previous
                        .into_iter()
                        .chain(next)
                        .any(|c| c.local_bearer == bearer)
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
    if let Some(connection) = next {
        toml::set_value(doc, &["mcp_servers", SERVER, "url"], super::URL)?;
        if needs_timeout {
            toml::set_value(doc, &["mcp_servers", SERVER, "tool_timeout_sec"], 240)?;
        }
        toml::set_value(
            doc,
            &["mcp_servers", SERVER, "http_headers", "Authorization"],
            format!("Bearer {}", connection.local_bearer),
        )
    } else {
        toml::remove(doc, &key)
    }
}
