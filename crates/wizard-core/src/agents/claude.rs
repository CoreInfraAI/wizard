mod video;
#[cfg(test)]
mod video_e2e;

use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, Agent, AgentInfo},
};
use crate::{
    config_files::{
        self,
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
    },
    platform::env_var_not_empty,
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use wizard_settings::Settings;

const HUB_URL: &str = "https://hub.coreinfra.ai/claude/api";
const API_URL: &str = "https://hub.coreinfra.ai/anthropic/subscription/api";
const NO_PROXY_SCRIPT: &str = "#!/bin/sh\n\nunset HTTPS_PROXY NO_PROXY\n";
const TOKEN_HEADER: &str = "X-CoreInfra-API-Key: ";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Claude {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
    pub video_needs_apply: bool,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<Claude> {
    let info = match detection::detect(Agent::Claude, settings.last_paths.claude.as_deref()) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let (routing_mode, token, video_needs_apply) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(Claude {
        info,
        routing_mode,
        token,
        video_needs_apply,
    })
}

fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = env_var_not_empty("CLAUDE_CONFIG_DIR") {
        return std::path::absolute(dir).context("failed to resolve Claude config directory");
    }
    Ok(dirs::home_dir()
        .context("failed to resolve home directory")?
        .join(".claude"))
}

fn global_config_path() -> Result<PathBuf> {
    if let Some(dir) = env_var_not_empty("CLAUDE_CONFIG_DIR") {
        return std::path::absolute(PathBuf::from(dir).join(".claude.json"))
            .context("failed to resolve Claude global config path");
    }
    Ok(dirs::home_dir()
        .context("failed to resolve home directory")?
        .join(".claude.json"))
}

/// The `CoreInfra` token in this agent's configuration, as detection reports it. Never log it.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

fn read_routing() -> Result<(RoutingMode, Option<AgentToken>, bool)> {
    let _guard = PreparedAgent::Claude.lock()?;
    let dir = config_dir()?;
    let settings = config_files::json::read(&dir.join("settings.json"))?;
    let mode = mode_from_config(&settings);
    let video_needs_apply = if mode == RoutingMode::Hub {
        let mut global = FileChange::read(global_config_path()?)?;
        video::configure(&dir, &mut global, &video_server()?, true).map_or(true, |files| {
            global.is_changed() || files.iter().any(FileChange::is_changed)
        })
    } else {
        false
    };
    Ok((mode, token_from_config(&settings, mode), video_needs_apply))
}

/// Reads only keys owned by the active mode: a disabled API key may belong to the user.
fn token_from_config(settings: &Value, mode: RoutingMode) -> Option<AgentToken> {
    let env = &settings["env"];
    let token = match mode {
        RoutingMode::Disabled => return None,
        RoutingMode::Hub => env["ANTHROPIC_API_KEY"].as_str()?,
        RoutingMode::Sub => env["ANTHROPIC_CUSTOM_HEADERS"]
            .as_str()?
            .lines()
            .find_map(|header| header.strip_prefix(TOKEN_HEADER))?,
    };
    AgentToken::new(token.to_owned())
}

fn mode_from_config(settings: &Value) -> RoutingMode {
    match settings["env"]["ANTHROPIC_BASE_URL"].as_str() {
        Some(HUB_URL) => RoutingMode::Hub,
        Some(API_URL) => RoutingMode::Sub,
        _ => RoutingMode::Disabled,
    }
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::new(AgentKind::Claude)?.get_backups()
}

pub fn restore_backup(id: u32) -> Result<()> {
    let manager = BackupManager::new(AgentKind::Claude)?;
    let plan = {
        let _guard = PreparedAgent::Claude.lock()?;
        prepare_restore_at(
            &config_dir()?,
            &global_config_path()?,
            &video_server()?,
            &manager,
            id,
        )?
    };
    plan.apply_with_backup(
        Some(manager),
        |_| bail!("Unexpected desktop proxy"),
        |_| bail!("Unexpected Codex proxy"),
    )
}

fn prepare_restore_at(
    dir: &Path,
    global_path: &Path,
    server: &Value,
    manager: &BackupManager,
    id: u32,
) -> Result<PreparedPlan> {
    let mut changes = manager.restore_changes(id)?;
    let path = dir.join("settings.json");
    let settings = if let Some(change) = changes.iter().find(|change| change.path() == &path) {
        config_files::json::parse(&change.after)?
    } else {
        config_files::json::read(&path)?
    };
    let mut global = FileChange::read(global_path.to_owned())?;
    changes.extend(video::configure(
        dir,
        &mut global,
        server,
        mode_from_config(&settings) == RoutingMode::Hub,
    )?);
    changes.push(global);
    Ok(prepare_plan(changes))
}

/// `token` is ignored when disabled.
pub fn prepare_routing(mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::Claude.lock()?;

    prepare_routing_at(
        &config_dir()?,
        &global_config_path()?,
        &video_server()?,
        mode,
        token,
    )
}

fn video_server() -> Result<Value> {
    video::server(crate::video::URL, &crate::video::install::headers_path()?)
}

fn prepare_routing_at(
    dir: &Path,
    global_path: &Path,
    server: &Value,
    mode: RoutingMode,
    token: &str,
) -> Result<PreparedPlan> {
    let script_path = dir.join("no-proxy.sh");

    let mut settings = FileChange::read(dir.join("settings.json"))?;
    let mut global = FileChange::read(global_path.to_owned())?;
    let mut script = FileChange::read(script_path.clone())?;

    let current_mode = mode_from_config(&config_files::json::parse(&settings.after)?);
    if !(mode == RoutingMode::Disabled && current_mode == RoutingMode::Disabled) {
        config_files::json::update(&mut settings.after, |settings| {
            let settings = settings
                .as_object_mut()
                .context("Claude settings must be an object")?;
            // Subscription sign-in loses to it; with Hub, Claude sends it to CoreInfra too.
            ensure!(
                mode == RoutingMode::Disabled || !settings.contains_key("apiKeyHelper"),
                "Remove apiKeyHelper from Claude Code settings.json: it overrides CoreInfra credentials"
            );
            let env = settings
                .entry("env")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("Claude env must be an object")?;
            remove_inactive_routing_settings(env, current_mode);
            ensure_no_conflicting_credentials(env, mode)?;
            write_routing_settings(env, mode, token, &script_path)
        })?;
    }
    if mode == RoutingMode::Hub {
        // The saved token is ASCII; Claude approves its last 20 characters.
        let suffix = token.get(token.len().saturating_sub(20)..).unwrap_or(token);
        approve_key(&mut global.after, suffix)?;
    }
    if mode == RoutingMode::Sub {
        script.after = FileSnapshot::new(NO_PROXY_SCRIPT.to_owned(), 0o700);
    }
    if mode != RoutingMode::Sub && current_mode == RoutingMode::Sub {
        script.after = FileSnapshot::Missing;
    }

    // Keep preparation files before activation, and script deletion after it.
    let mut changes = video::configure(dir, &mut global, server, mode == RoutingMode::Hub)?;
    changes.extend(if mode == RoutingMode::Sub {
        [global, script, settings]
    } else {
        [global, settings, script]
    });
    Ok(prepare_plan(changes))
}

fn prepare_plan(changes: Vec<FileChange>) -> PreparedPlan {
    let steps = changes
        .into_iter()
        .map(|change| PreparedStep::ChangeFile { change })
        .collect();
    PreparedPlan::new(PreparedAgent::Claude, steps)
}

fn remove_inactive_routing_settings(env: &mut Map<String, Value>, current_mode: RoutingMode) {
    // Only clean up fields owned by the previously selected CoreInfra mode.
    let previous_keys: &[&str] = match current_mode {
        RoutingMode::Disabled => &[],
        RoutingMode::Hub => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
            "ANTHROPIC_DEFAULT_MODEL",
        ],
        RoutingMode::Sub => &[
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "HTTPS_PROXY",
            "NO_PROXY",
            "CLAUDE_ENV_FILE",
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
            "ANTHROPIC_DEFAULT_MODEL",
        ],
    };
    for key in previous_keys {
        env.shift_remove(*key);
    }
}

/// The user's own credentials are kept, but Claude Code prefers them to the ones a mode writes.
fn ensure_no_conflicting_credentials(env: &Map<String, Value>, mode: RoutingMode) -> Result<()> {
    // A cloud provider ignores ANTHROPIC_BASE_URL, so no request would reach CoreInfra.
    let conflicts: &[&str] = match mode {
        RoutingMode::Disabled => &[],
        RoutingMode::Hub => &[
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ],
        RoutingMode::Sub => &[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ],
    };
    if let Some(key) = conflicts.iter().find(|key| env.contains_key(**key)) {
        bail!(
            "Remove {key} from the env section of Claude Code settings.json: it overrides CoreInfra credentials"
        );
    }
    Ok(())
}

fn write_routing_settings(
    env: &mut Map<String, Value>,
    mode: RoutingMode,
    token: &str,
    script_path: &Path,
) -> Result<()> {
    if mode != RoutingMode::Disabled {
        // Off by default behind a custom base URL; without it Anthropic buffers whole tool inputs.
        env.insert(
            "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING".into(),
            json!("1"),
        );
        // Lowest-priority default: `/model`, `--model` and `ANTHROPIC_MODEL` still win.
        env.insert("ANTHROPIC_DEFAULT_MODEL".into(), json!("sonnet"));
    }

    match mode {
        RoutingMode::Disabled => {}
        RoutingMode::Hub => {
            env.insert("ANTHROPIC_BASE_URL".into(), json!(HUB_URL));
            env.insert("ANTHROPIC_API_KEY".into(), json!(token));
            env.insert(
                "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
                json!("1"),
            );
        }
        RoutingMode::Sub => {
            // Encode the password component; the header keeps the literal token.
            let password: String = token
                .bytes()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                        char::from(byte).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect();
            env.insert("ANTHROPIC_BASE_URL".into(), json!(API_URL));
            env.insert(
                "ANTHROPIC_CUSTOM_HEADERS".into(),
                json!(format!("{TOKEN_HEADER}{token}")),
            );
            env.insert(
                "HTTPS_PROXY".into(),
                json!(format!(
                    "https://coreinfra:{password}@proxy.hub.coreinfra.ai:443"
                )),
            );
            env.insert(
                "NO_PROXY".into(),
                json!("localhost,127.0.0.1,::1,hub.coreinfra.ai"),
            );
            env.insert(
                "CLAUDE_ENV_FILE".into(),
                json!(
                    script_path
                        .to_str()
                        .context("Claude script path is not valid UTF-8")?
                ),
            );
        }
    }
    Ok(())
}

fn approve_key(after: &mut FileSnapshot, suffix: &str) -> Result<()> {
    config_files::json::update(after, |config| {
        config["hasCompletedOnboarding"] = json!(true);
        let approved = config
            .as_object_mut()
            .context("Claude config must be an object")?
            .entry("customApiKeyResponses")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("Claude customApiKeyResponses must be an object")?
            .entry("approved")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Claude approved keys must be an array")?;
        if !approved.iter().any(|value| value.as_str() == Some(suffix)) {
            approved.push(json!(suffix));
        }
        // Preserve old approvals and onboarding when switching away from Hub.
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_follows_gateway_mode_and_backup_restores_preserve_other_servers() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join(".claude");
        let global = temp.path().join(".claude.json");
        let skill = dir.join("skills/coreinfra-video/SKILL.md");
        let registration =
            video::server(crate::video::URL, &temp.path().join("video headers.json"))?;
        let manager =
            || BackupManager::in_directory(AgentKind::Claude, temp.path().join("backups"));
        let apply = |plan: PreparedPlan| {
            plan.apply_with_backup(
                Some(manager()),
                |_| bail!("Unexpected desktop proxy"),
                |_| bail!("Unexpected Codex proxy"),
            )
        };
        let prepare =
            |mode| prepare_routing_at(&dir, &global, &registration, mode, "inference-key");
        let read_global = || config_files::json::read(&global);
        let foreign = json!({"type":"http","url":"https://example.org/mcp"});
        std::fs::write(
            &global,
            json!({"mcpServers":{"other":foreign},"theme":"dark"}).to_string(),
        )?;

        let first = prepare(RoutingMode::Hub)?;
        assert!(!skill.exists());
        apply(first)?;
        assert!(skill.is_file());
        assert_eq!(
            read_global()?["mcpServers"]["coreinfra_video"],
            registration
        );
        assert!(!registration.to_string().contains("inference-key"));
        assert!(prepare(RoutingMode::Hub)?.steps().iter().all(
            |step| matches!(step, PreparedStep::ChangeFile { change } if !change.is_changed())
        ));
        assert_eq!(manager().get_backups()?.len(), 1);

        // Claude itself can change its global state between configuration and restore.
        let mut updated = read_global()?;
        updated["theme"] = json!("light");
        std::fs::write(&global, updated.to_string())?;
        apply(prepare_restore_at(
            &dir,
            &global,
            &registration,
            &manager(),
            1,
        )?)?;
        assert!(!skill.exists());
        assert_eq!(read_global()?["theme"], "light");
        assert_eq!(read_global()?["mcpServers"], json!({"other":foreign}));

        apply(prepare(RoutingMode::Hub)?)?;
        std::fs::write(&skill, "my edited skill")?;
        assert!(prepare(RoutingMode::Hub).is_err());
        for mode in [RoutingMode::Sub, RoutingMode::Disabled] {
            apply(prepare(mode)?)?;
            assert_eq!(std::fs::read_to_string(&skill)?, "my edited skill");
            assert_eq!(read_global()?["mcpServers"], json!({"other":foreign}));
        }
        Ok(())
    }

    #[test]
    fn foreign_video_entries_block_install_without_changing_any_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("claude");
        let global = temp.path().join("global.json");
        let server = video::server(crate::video::URL, &temp.path().join("headers.json"))?;
        let existing = json!({"mcpServers":{"coreinfra_video":{"type":"http","url":crate::video::URL,"headersHelper":"user-helper"}}});
        std::fs::write(&global, existing.to_string())?;
        assert!(prepare_routing_at(&dir, &global, &server, RoutingMode::Hub, "key").is_err());
        assert!(!dir.exists());
        assert_eq!(config_files::json::read(&global)?, existing);
        let plan = prepare_routing_at(&dir, &global, &server, RoutingMode::Disabled, "")?;
        assert!(plan.steps().iter().all(
            |step| matches!(step, PreparedStep::ChangeFile { change } if !change.is_changed())
        ));
        Ok(())
    }

    #[test]
    fn user_credentials_that_override_a_mode_are_rejected() {
        let env = |key: &str| {
            let mut env = Map::new();
            env.insert(key.to_owned(), json!("user"));
            env
        };
        let api_key = env("ANTHROPIC_API_KEY");
        let auth_token = env("ANTHROPIC_AUTH_TOKEN");
        assert!(ensure_no_conflicting_credentials(&api_key, RoutingMode::Sub).is_err());
        // Hub replaces ANTHROPIC_API_KEY with its own.
        assert!(ensure_no_conflicting_credentials(&api_key, RoutingMode::Hub).is_ok());
        assert!(ensure_no_conflicting_credentials(&auth_token, RoutingMode::Hub).is_err());
        assert!(ensure_no_conflicting_credentials(&auth_token, RoutingMode::Sub).is_err());
        assert!(ensure_no_conflicting_credentials(&auth_token, RoutingMode::Disabled).is_ok());
        let bedrock = env("CLAUDE_CODE_USE_BEDROCK");
        assert!(ensure_no_conflicting_credentials(&bedrock, RoutingMode::Hub).is_err());
        assert!(ensure_no_conflicting_credentials(&bedrock, RoutingMode::Sub).is_err());
    }
}
