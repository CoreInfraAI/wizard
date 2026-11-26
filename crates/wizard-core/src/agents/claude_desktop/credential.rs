//! Claude's gateway helper and the media daemon use the same profile-specific credential.
use super::{AgentToken, applied_id, config_library, has_hub_connection};
use crate::config_files::{
    changes::{FileChange, FileSnapshot},
    json,
};
use anyhow::{Context as _, Result, ensure};
use serde_json::{Map, Value, json};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

pub const ARG: &str = "--claude-desktop-credential";
pub(crate) const DIRECTORY: &str = "claude-desktop-credentials";

fn usable(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with("__sealed__:")
        && !token.chars().any(char::is_whitespace)
}

fn args(id: &str, digest: &str) -> Value {
    json!([ARG, id, digest])
}

pub(super) fn path(dir: &Path, id: &str) -> Result<PathBuf> {
    // Use Wizard's directory under the same platform-local app data root as Claude-3p.
    // Callers obtain id from applied_id or generate a canonical UUID themselves.
    Ok(dir
        .parent()
        .context("Missing Claude Desktop configuration directory")?
        .with_file_name(wizard_settings::paths::IDENTIFIER)
        .join(DIRECTORY)
        .join(format!("{id}.key")))
}

fn helper_matches(profile: &Value, saved_helper: &Value, windows: bool) -> bool {
    saved_helper.as_str().is_some_and(|path| !path.is_empty())
        && profile["inferenceCredentialHelper"] == *saved_helper
        && (!windows
            || profile
                .get("inferenceCredentialHelperWindows")
                .is_none_or(|override_helper| override_helper == saved_helper))
}

/// Stage the credential together with the profile so applying, rotating and restoring
/// it uses the same conflict checks and protected backups as other agent credentials.
pub(super) fn prepare(
    dir: &Path,
    id: &str,
    helper: &Path,
    token: Option<&str>,
) -> Result<FileChange> {
    let mut change = FileChange::read(path(dir, id)?)?;
    change.after = if let Some(token) = token {
        ensure!(
            usable(token),
            "Выберите ключ CoreInfra Hub для Claude Desktop и нажмите «Применить»"
        );
        FileSnapshot::new(json!({"helper": helper, "token": token}).to_string(), 0o600)
    } else {
        FileSnapshot::Missing
    };
    Ok(change)
}

pub(super) fn configure(profile: &mut Map<String, Value>, id: &str, helper: &Path, token: &str) {
    profile.insert("inferenceCredentialKind".into(), json!("helper-script"));
    profile.insert("inferenceCredentialHelper".into(), json!(helper));
    profile.insert(
        "inferenceCredentialHelperArgs".into(),
        args(id, &crate::media::fingerprint(token)),
    );
}

pub(super) fn profile_token(
    dir: &Path,
    id: &str,
    profile: &Value,
    expected_helper: Option<&Path>,
) -> Result<(Option<AgentToken>, bool)> {
    profile_token_for_platform(dir, id, profile, expected_helper, cfg!(windows))
}

pub(super) fn profile_token_for_platform(
    dir: &Path,
    id: &str,
    profile: &Value,
    expected_helper: Option<&Path>,
    windows: bool,
) -> Result<(Option<AgentToken>, bool)> {
    match profile["inferenceCredentialKind"].as_str() {
        None | Some("static") => Ok((
            profile["inferenceGatewayApiKey"]
                .as_str()
                .filter(|token| usable(token))
                .and_then(|token| AgentToken::new(token.to_owned())),
            true,
        )),
        Some("helper-script") => {
            let snapshot = FileSnapshot::read(&path(dir, id)?)?;
            // An incomplete credential must leave the profile detectable so Apply can repair it.
            let Ok(saved) = json::parse(&snapshot) else {
                return Ok((None, true));
            };
            let Some(token) = saved["token"].as_str().filter(|token| usable(token)) else {
                return Ok((None, true));
            };
            let digest = crate::media::fingerprint(token);
            if !helper_matches(profile, &saved["helper"], windows)
                || profile["inferenceCredentialHelperArgs"] != args(id, &digest)
            {
                return Ok((None, true));
            }
            let helper_moved = expected_helper
                .is_some_and(|helper| profile["inferenceCredentialHelper"] != json!(helper));
            Ok((AgentToken::new(token.to_owned()), helper_moved))
        }
        _ => Ok((None, true)),
    }
}

pub(super) fn helper_token(dir: &Path, id: &str, digest: &str) -> Result<AgentToken> {
    let meta = json::read(&dir.join("_meta.json"))?;
    ensure!(
        applied_id(&meta)? == Some(id),
        "Claude Desktop profile selection has changed"
    );
    let profile = json::read(&dir.join(format!("{id}.json")))?;
    ensure!(
        json::read(&dir.with_file_name("claude_desktop_config.json"))?["deploymentMode"] != "1p"
            && has_hub_connection(&profile)
            && profile["inferenceCredentialKind"] == "helper-script"
            && profile["inferenceCredentialHelperArgs"] == args(id, digest),
        "Claude Desktop gateway credential selection has changed"
    );
    profile_token(dir, id, &profile, None)?
        .0
        .context("Reapply the Claude Desktop Hub key in CoreInfra Wizard")
}

/// Claude launches this exact entry point with a profile ID and public key fingerprint.
pub fn run(id: &str, digest: &str) -> Result<()> {
    let token = helper_token(&config_library()?, id, digest)?;
    writeln!(std::io::stdout().lock(), "{}", token.into_secret())?;
    Ok(())
}
