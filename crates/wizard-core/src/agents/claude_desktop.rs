pub mod credential;
mod media;
mod models;
pub use models::Models;

use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, AgentInfo, DesktopAgent},
};
use crate::{
    config_files::{
        backups::{AgentKind, BackupManager},
        changes::{FileChange, FileSnapshot},
        json,
    },
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use wizard_settings::Settings;

const HUB_URL: &str = "https://hub.coreinfra.ai/claude-desktop/api";
// Read-only compatibility with an unreleased configuration. Never bind this port.
const RETIRED_PROXY_URL: &str = "http://127.0.0.1:33789";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ClaudeDesktop {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
    pub configuration_needs_apply: bool,
    pub media_needs_apply: bool,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<ClaudeDesktop> {
    let info = match detection::detect_desktop(
        DesktopAgent::Claude,
        settings.last_paths.claude_desktop.as_deref(),
    ) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };

    let configuration = || -> Result<ClaudeDesktop> {
        let _guard = PreparedAgent::ClaudeDesktop.lock()?;
        let dir = config_library()?;
        let registration = media::Registration::for_wizard()?;
        let (routing_mode, token, configuration_needs_apply) =
            read_profile_with_helper(&dir, Some(&registration.command))?;
        let plan = prepare_with_media(
            &dir,
            &config_1p_path()?,
            None,
            settings.media.claude_desktop.is_some(),
            &registration,
            None,
        );
        Ok(ClaudeDesktop {
            info,
            routing_mode,
            token,
            configuration_needs_apply,
            media_needs_apply: plan.map_or(true, |plan| !plan.info().is_empty()),
        })
    };
    match configuration() {
        Ok(desktop) => AgentDetection::Found(desktop),
        Err(error) => AgentDetection::Error(format!("{error:#}")),
    }
}

/// The 1p configuration, where Claude Desktop in 1p mode reads local MCP servers.
fn config_1p_path() -> Result<PathBuf> {
    // macOS: ~/Library/Application Support, Windows: %APPDATA%, Linux: ~/.config.
    std::path::absolute(
        dirs::config_dir()
            .context("failed to resolve Claude Desktop configuration directory")?
            .join("Claude/claude_desktop_config.json"),
    )
    .context("failed to resolve Claude Desktop configuration path")
}

/// Reads the 1p configuration now; call it while Claude Desktop is closed when it may run.
/// Only the Wizard-owned `coreinfra_media` entry changes.
pub(crate) fn prepare_1p_media(
    path: PathBuf,
    enable: bool,
    headers: PathBuf,
) -> Result<FileChange> {
    media_1p_change(path, enable, &media::Registration::with_headers(headers)?)
}

fn media_1p_change(
    path: PathBuf,
    enable: bool,
    registration: &media::Registration,
) -> Result<FileChange> {
    let mut change = FileChange::read(path)?;
    if !enable && change.before().content().is_none() {
        return Ok(change);
    }
    json::update(&mut change.after, |config| {
        registration.update_1p_config(
            config
                .as_object_mut()
                .context("Claude Desktop configuration must be an object")?,
            enable,
        )
    })?;
    Ok(change)
}

fn config_library() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let directory = dirs::home_dir().map(|home| home.join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let directory = dirs::data_local_dir();
    #[cfg(target_os = "linux")]
    let directory = dirs::config_dir();

    std::path::absolute(
        directory
            .context("failed to resolve Claude Desktop configuration directory")?
            .join("Claude-3p/configLibrary"),
    )
    .context("failed to resolve Claude Desktop configuration path")
}

fn applied_id(meta: &Value) -> Result<Option<&str>> {
    ensure!(
        meta.get("hybridPointer").is_none_or(Value::is_null),
        "Claude Desktop uses an organization configuration; switch to a local profile first"
    );

    let id = match meta.get("appliedId") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(id)) => id,
        Some(_) => anyhow::bail!("invalid Claude Desktop applied profile identifier"),
    };

    // Match the canonical IDs used by Claude; never interpret an ID as a path.
    let valid = uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == *id);
    ensure!(valid, "invalid Claude Desktop applied profile identifier");
    Ok(Some(id))
}

fn has_hub_connection(config: &Value) -> bool {
    config["inferenceProvider"] == "gateway"
        && config["inferenceGatewayBaseUrl"]
            .as_str()
            .is_some_and(|url| {
                // Manual setup also uses the Claude API alias for the same Hub gateway.
                matches!(
                    url.trim_end_matches('/'),
                    HUB_URL
                        | "https://hub.coreinfra.ai/claude/api"
                        | "https://hub.coreinfra.ai/anthropic/api"
                )
            })
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::new(AgentKind::ClaudeDesktop)?.get_backups()
}

pub fn prepare_restore_backup(id: u32, media: bool) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
    prepare_restore_with_media(
        &config_library()?,
        &config_1p_path()?,
        &BackupManager::new(AgentKind::ClaudeDesktop)?,
        id,
        media,
        &media::Registration::for_wizard()?,
    )
}

#[cfg(test)]
fn prepare_restore_at(dir: &Path, manager: &BackupManager, id: u32) -> Result<PreparedPlan> {
    let config_1p = dir
        .parent()
        .and_then(Path::parent)
        .context("Missing test profile directory")?
        .join("Claude/claude_desktop_config.json");
    prepare_restore_with_media(
        dir,
        &config_1p,
        manager,
        id,
        false,
        &tests::registration(dir)?,
    )
}

fn prepare_restore_with_media(
    dir: &Path,
    config_1p: &Path,
    manager: &BackupManager,
    id: u32,
    media_grant: bool,
    media: &media::Registration,
) -> Result<PreparedPlan> {
    let mut changes = manager.restore_changes(id)?;
    // An old backup must not point Claude at a listener that no longer exists.
    for change in &mut changes {
        if change.path().parent() == Some(dir)
            && let Ok(profile) = json::parse(&change.after)
            && profile["egressProxyUrl"] == RETIRED_PROXY_URL
        {
            json::update(&mut change.after, |profile| {
                profile["egressProxyUrl"] = json!(format!("http://{}", wizard_proxy::ADDRESS));
                Ok(())
            })?;
        }
    }
    // A restore also backs up the state it replaces when it differs from last_written.
    let meta_path = dir.join("_meta.json");
    let current_meta = changes
        .iter()
        .find(|change| change.path() == &meta_path)
        .context("Claude Desktop backup is missing profile metadata")?;
    // Corrupt current metadata must not prevent recovery from a valid backup.
    let current = json::parse(current_meta.before()).ok();
    let current_id = current
        .as_ref()
        .and_then(|meta| applied_id(meta).ok().flatten());
    let restored = |changes: &[FileChange], path: &Path| -> Result<Value> {
        let change = changes
            .iter()
            .find(|change| change.path() == path)
            .context("Claude Desktop backup is missing a configuration file")?;
        json::parse(&change.after)
    };
    let meta = restored(&changes, &meta_path)?;
    let selected = if meta.get("hybridPointer").is_none_or(Value::is_null) {
        applied_id(&meta)?.map(str::to_owned)
    } else {
        None
    };
    // Legacy backups have no `.key` entries. Clear the sidecar for every profile
    // snapshot in the restore set (including deleted profiles).
    let restored_ids: Vec<_> = changes
        .iter()
        .filter(|change| change.path().parent() == Some(dir))
        .filter_map(|change| {
            let id = change.path().file_name()?.to_str()?.strip_suffix(".json")?;
            uuid::Uuid::parse_str(id)
                .is_ok_and(|uuid| uuid.to_string() == id)
                .then(|| id.to_owned())
        })
        .collect();
    for profile_id in restored_ids {
        let path = credential::path(dir, &profile_id)?;
        if !changes.iter().any(|change| change.path() == &path) {
            let mut change = FileChange::read(path)?;
            change.after = FileSnapshot::Missing;
            changes.push(change);
        }
    }
    // Include the current profile and key so that if restore creates a backup,
    // the displaced selection can be restored later.
    if let Some(id) = current_id {
        for path in [dir.join(format!("{id}.json")), credential::path(dir, id)?] {
            if !changes.iter().any(|change| change.path() == &path) {
                changes.push(FileChange::read(path)?);
            }
        }
    }
    let preferences_path = dir.with_file_name("claude_desktop_config.json");
    let use_3p = selected.is_some()
        && restored(&changes, &preferences_path)
            .map_or(true, |prefs| prefs["deploymentMode"] != "1p");
    // Reconcile the restored profile with the current media grant so a 1.0.10 backup does not
    // reinstate a legacy connector pointing to a migrated/deleted headers file.
    if let Some(active_id) = &selected {
        let active_path = dir.join(format!("{active_id}.json"));
        if let Some(change) = changes
            .iter_mut()
            .find(|change| change.path() == &active_path)
            && change.after.content().is_some()
        {
            json::update(&mut change.after, |profile| {
                let object = profile
                    .as_object_mut()
                    .context("Claude Desktop configuration must be an object")?;
                media.update_3p_profile(object, media_grant && use_3p)
            })?;
        }
    }
    let proxy_enabled = if let Some(active_id) = &selected {
        let profile = restored(&changes, &dir.join(format!("{active_id}.json")))?;
        profile["egressProxyUrl"] == format!("http://{}", wizard_proxy::ADDRESS)
            && profile.get("egressProxyPacUrl").is_none_or(Value::is_null)
    } else {
        false
    };
    let mut steps: Vec<_> = crate::media::install::migrate_headers(&media.headers, media_grant)?
        .into_iter()
        .chain(changes)
        .map(|change| PreparedStep::ChangeFile { change })
        .collect();
    let enable_1p = media_grant && !use_3p;
    if media_1p_change(config_1p.to_owned(), enable_1p, media)?.is_changed() {
        steps.push(PreparedStep::SetClaudeDesktop1pMedia {
            path: config_1p.to_owned(),
            enabled: enable_1p,
            headers: media.headers.clone(),
        });
    }
    steps.push(PreparedStep::SetClaudeProxyEnabled {
        enabled: proxy_enabled,
    });
    Ok(PreparedPlan::new(PreparedAgent::ClaudeDesktop, steps))
}

/// The key for paid media calls: the profile's key in Hub; in Sub, which has no agent key,
/// Wizard's own proxy key from its personal Hub. Never log it.
pub(crate) fn media_token() -> Result<Option<String>> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
    let (mode, token, _) = read_profile(&config_library()?)?;
    Ok(match mode {
        RoutingMode::Hub => token.map(AgentToken::into_secret),
        RoutingMode::Sub => wizard_settings::load_from_file()?
            .proxy_token
            .filter(|token| !token.is_empty()),
        RoutingMode::Disabled => None,
    })
}

fn read_profile(dir: &Path) -> Result<(RoutingMode, Option<AgentToken>, bool)> {
    read_profile_with_helper(dir, None)
}

fn read_profile_with_helper(
    dir: &Path,
    expected_helper: Option<&Path>,
) -> Result<(RoutingMode, Option<AgentToken>, bool)> {
    let meta = json::read(&dir.join("_meta.json"))?;
    let Some(id) = applied_id(&meta)? else {
        return Ok((RoutingMode::Disabled, None, false));
    };

    let config = json::read(&dir.join(format!("{id}.json")))?;
    let preferences = json::read(&dir.with_file_name("claude_desktop_config.json"))?;
    let proxy_url = format!("http://{}", wizard_proxy::ADDRESS);
    let proxy_needs_apply = config["egressProxyUrl"] == RETIRED_PROXY_URL;

    if preferences["deploymentMode"] == "1p"
        && (config["egressProxyUrl"] == proxy_url || proxy_needs_apply)
        && config.get("egressProxyPacUrl").is_none()
    {
        Ok((RoutingMode::Sub, None, proxy_needs_apply))
    } else if preferences["deploymentMode"] != "1p" && has_hub_connection(&config) {
        let (token, credential_needs_apply) =
            credential::profile_token(dir, id, &config, expected_helper)?;
        let configuration_needs_apply = token.is_none()
            || credential_needs_apply
            || proxy_needs_apply
            || config["inferenceGatewayBaseUrl"]
                .as_str()
                .map(|url| url.trim_end_matches('/'))
                != Some(HUB_URL)
            || Models::from_value(&config["inferenceModels"])
                .is_none_or(|models| models.default_model_index() != 0);
        Ok((RoutingMode::Hub, token, configuration_needs_apply))
    } else {
        Ok((RoutingMode::Disabled, None, false))
    }
}

/// Prepare a local profile update without writing files or restarting Claude. Hub writes `token`
/// to the profile; Hub and Sub enable local proxy demand; Disabled ignores it.
/// `media`: whether Wizard holds a media grant for Claude Desktop.
pub fn prepare_routing(
    mode: RoutingMode,
    token: &str,
    media: bool,
    models: Option<&Models>,
) -> Result<PreparedPlan> {
    prepare_configuration(Some((mode, token)), media, models)
}

/// Reconcile media in the current deployment without changing inference or proxy settings.
pub fn prepare_media(enabled: bool) -> Result<PreparedPlan> {
    prepare_configuration(None, enabled, None)
}

fn prepare_configuration(
    routing: Option<(RoutingMode, &str)>,
    media: bool,
    models: Option<&Models>,
) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    prepare_with_media(
        &config_library()?,
        &config_1p_path()?,
        routing,
        media,
        &media::Registration::for_wizard()?,
        models,
    )
}

#[cfg(test)]
fn prepare_routing_at(dir: &Path, mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    let config_1p = dir
        .parent()
        .and_then(Path::parent)
        .context("Missing test profile directory")?
        .join("Claude/claude_desktop_config.json");
    prepare_with_media(
        dir,
        &config_1p,
        Some((mode, token)),
        false,
        &tests::registration(dir)?,
        Some(&tests::fixture_models()),
    )
}

fn prepare_with_media(
    dir: &Path,
    config_1p: &Path,
    routing: Option<(RoutingMode, &str)>,
    media_grant: bool,
    media: &media::Registration,
    models: Option<&Models>,
) -> Result<PreparedPlan> {
    let mut meta = FileChange::read(dir.join("_meta.json"))?;
    let metadata = json::parse(&meta.after)?;
    let selected = applied_id(&metadata)?;

    let mut preferences = FileChange::read(dir.with_file_name("claude_desktop_config.json"))?;
    let use_3p = match routing {
        Some((mode, _)) => mode == RoutingMode::Hub,
        None => selected.is_some() && json::parse(&preferences.after)?["deploymentMode"] != "1p",
    };
    let mut id = selected.map(str::to_owned);
    if id.is_none() && routing.is_some_and(|(mode, _)| mode != RoutingMode::Disabled) {
        let new_id = uuid::Uuid::new_v4().to_string();
        json::update(&mut meta.after, |meta| {
            let entries = meta
                .as_object_mut()
                .context("Claude Desktop metadata must be an object")?
                .entry("entries")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("Claude Desktop profile entries must be an array")?;
            ensure!(
                !entries.iter().any(|entry| entry["id"] == new_id),
                "new Claude Desktop profile identifier already exists; prepare again"
            );

            entries.push(json!({"id": new_id, "name": "CoreInfra"}));
            meta["appliedId"] = json!(new_id);
            Ok(())
        })?;
        id = Some(new_id);
    }

    let mut steps: Vec<_> = crate::media::install::migrate_headers(&media.headers, media_grant)?
        .into_iter()
        .map(|change| PreparedStep::ChangeFile { change })
        .collect();
    if let Some(id) = id {
        let mut config = FileChange::read(dir.join(format!("{id}.json")))?;
        ensure!(
            selected.is_some() || config.before().content().is_none(),
            "new Claude Desktop profile already exists; prepare again"
        );
        // Include the credential even on media-only changes so a backup of a helper
        // profile can always restore the corresponding key.
        let credential_change = if let Some((mode, token)) = routing {
            credential::prepare(
                dir,
                &id,
                &media.command,
                (mode == RoutingMode::Hub).then_some(token),
            )?
        } else {
            FileChange::read(credential::path(dir, &id)?)?
        };
        steps.push(PreparedStep::ChangeFile {
            change: credential_change,
        });
        json::update(&mut config.after, |config| {
            let object = config
                .as_object_mut()
                .context("Claude Desktop configuration must be an object")?;

            media.update_3p_profile(object, media_grant && use_3p)?;
            let Some((mode, token)) = routing else {
                return Ok(());
            };
            // The complete previous profile is backed up before applying this plan.
            // Clear provider-specific credentials and proxy overrides before setting the mode.
            object
                .retain(|key, _| !key.starts_with("inference") && !key.starts_with("egressProxy"));
            if mode == RoutingMode::Hub {
                // An unset key enables Auto only in Code; Cowork needs an explicit opt-in.
                object.entry("autoModeEnabled").or_insert(json!(true));
                object.insert("inferenceProvider".into(), json!("gateway"));
                credential::configure(object, &id, &media.command, token);
                object.insert("inferenceGatewayBaseUrl".into(), json!(HUB_URL));
                object.insert("inferenceGatewayAuthScheme".into(), json!("x-api-key"));
                object.insert(
                    "inferenceModels".into(),
                    models
                        .context("Каталог моделей Claude Desktop не загружен")?
                        .value()?,
                );
            }

            if mode == RoutingMode::Sub {
                object.insert("disableAutoUpdates".into(), json!(true));
            } else {
                object.shift_remove("disableAutoUpdates");
            }

            if mode != RoutingMode::Disabled {
                object.insert(
                    "egressProxyUrl".into(),
                    json!(format!("http://{}", wizard_proxy::ADDRESS)),
                );
            }
            Ok(())
        })?;
        steps.push(PreparedStep::ChangeFile { change: config });
    }

    // Disabled returns to the Claude account (1p), where sign-in works.
    if routing.is_some() {
        json::update(&mut preferences.after, |preferences| {
            preferences["deploymentMode"] = json!(if use_3p { "3p" } else { "1p" });
            Ok(())
        })?;
    }

    // Publish a newly created profile before selecting it. Keep unchanged steps too.
    steps.extend([
        PreparedStep::ChangeFile { change: meta },
        PreparedStep::ChangeFile {
            change: preferences,
        },
    ]);
    // Claude Desktop rewrites its 1p configuration while running: read it again after
    // closing the app if the registration needs to change.
    let enabled = media_grant && !use_3p;
    if media_1p_change(config_1p.to_owned(), enabled, media)?.is_changed() {
        steps.push(PreparedStep::SetClaudeDesktop1pMedia {
            path: config_1p.to_owned(),
            enabled,
            headers: media.headers.clone(),
        });
    }
    if let Some((mode, _)) = routing {
        steps.push(PreparedStep::SetClaudeProxyEnabled {
            enabled: mode != RoutingMode::Disabled,
        });
    }
    Ok(PreparedPlan::new(PreparedAgent::ClaudeDesktop, steps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_files::changes::FileSnapshot;

    /// Keeps the grant file next to the test profile.
    pub(super) fn registration(dir: &Path) -> Result<media::Registration> {
        media::Registration::with_headers(dir.join("media-claude-desktop-headers.json"))
    }

    pub(super) fn fixture_models() -> Models {
        Models::from_value(&json!([
            {"name":"claude-sonnet-5", "labelOverride":"Claude Sonnet 5"},
            {"name":"claude-coreinfra-cn-3", "labelOverride":"GLM 5.3", "behavesAs":"claude-sonnet-4-6", "maxEffort":"max"},
            {"name":"claude-coreinfra-cn-1", "labelOverride":"DeepSeek V4.1 Flash", "behavesAs":"claude-sonnet-4-6", "maxEffort":"max"}
        ])).expect("valid model fixture")
    }

    #[test]
    fn first_launch_needs_only_precreated_configuration() -> Result<()> {
        for mode in [RoutingMode::Hub, RoutingMode::Sub] {
            let root = tempfile::tempdir()?;
            let dir = root.path().join("Claude-3p/configLibrary");
            let plan = prepare_routing_at(&dir, mode, "test-agent-token")?;
            assert!(!dir.parent().expect("profile directory").exists());
            let mut activated = false;
            plan.apply_with_backup(
                None,
                |enabled| {
                    assert!(enabled);
                    // All files must be present before starting the proxy or the desktop app.
                    let meta = json::read(&dir.join("_meta.json"))?;
                    let id = applied_id(&meta)?.expect("selected profile");
                    assert_eq!(meta["entries"], json!([{ "id": id, "name": "CoreInfra" }]));
                    let profile = json::read(&dir.join(format!("{id}.json")))?;
                    assert_eq!(profile["egressProxyUrl"], "http://127.0.0.1:33787");
                    let preferences =
                        json::read(&dir.with_file_name("claude_desktop_config.json"))?;
                    assert_eq!(
                        preferences["deploymentMode"],
                        if mode == RoutingMode::Hub { "3p" } else { "1p" }
                    );
                    if mode == RoutingMode::Sub {
                        assert!(profile.get("inferenceGatewayApiKey").is_none());
                    } else {
                        assert_eq!(profile["inferenceModels"], fixture_models().value()?);
                        assert_eq!(profile["autoModeEnabled"], true);
                    }
                    activated = true;
                    Ok(())
                },
                |_| panic!("unexpected ChatGPT proxy"),
            )?;
            assert!(activated);
            let (detected, token, needs_apply) = read_profile(&dir)?;
            assert_eq!(detected, mode);
            assert!(!needs_apply);
            assert_eq!(
                token.map(AgentToken::into_secret).as_deref(),
                (mode == RoutingMode::Hub).then_some("test-agent-token")
            );

            let repeated = prepare_routing_at(&dir, mode, "test-agent-token")?;
            assert!(repeated.steps().iter().all(|step| match step {
                PreparedStep::ChangeFile { change } => !change.is_changed(),
                _ => true,
            }));
            assert_eq!(std::fs::read_dir(&dir)?.count(), 2);
        }
        Ok(())
    }

    struct Fixture {
        root: tempfile::TempDir,
        dir: PathBuf,
        files: Vec<PathBuf>,
    }

    impl Fixture {
        fn new(provider: &str) -> Result<Self> {
            let root = tempfile::tempdir()?;
            let dir = root.path().join("Claude-3p/configLibrary");
            std::fs::create_dir_all(&dir)?;
            let id = uuid::Uuid::new_v4().to_string();
            let files = vec![
                dir.join(format!("{id}.json")),
                dir.join("_meta.json"),
                dir.with_file_name("claude_desktop_config.json"),
            ];
            let contents = [
                json!({
                    "inferenceProvider": provider,
                    "inferenceCredentialKind": "command",
                    "inferenceGatewayBaseUrl": "https://gateway.example/api",
                    "inferenceGatewayApiKey": "old-key",
                    "inferenceGatewayAuthScheme": "bearer",
                    "inferenceCredentialCommand": "old-command",
                    "inferenceBedrockRegion": "us-east-1",
                    "inferenceModels": ["old-model"],
                    "egressProxyUrl": "http://corp:3128",
                    "egressProxyPacUrl": "https://corp/proxy.pac",
                    "egressProxyAuth": "old-proxy-auth",
                    "disableAutoUpdates": true,
                    "managedMcpServers": [{"name":"example", "transport":"http", "url":"https://mcp.example"}],
                    "autoModeEnabled": true,
                }),
                json!({"appliedId": id, "entries": [{"id": id, "name": "Manual"}]}),
                json!({"deploymentMode": "3p", "unrelated": true}),
            ];
            for (path, value) in files.iter().zip(contents) {
                std::fs::write(path, serde_json::to_string_pretty(&value)?)?;
            }
            Ok(Self { root, dir, files })
        }

        fn manager(&self) -> BackupManager {
            BackupManager::in_directory(AgentKind::ClaudeDesktop, self.root.path().join("backups"))
        }

        fn snapshots(&self) -> Result<Vec<FileSnapshot>> {
            self.files
                .iter()
                .map(|path| FileSnapshot::read(path))
                .collect()
        }

        fn apply(&self, plan: PreparedPlan, expected_proxy: bool) -> Result<()> {
            let mut called = false;
            plan.apply_with_backup(
                Some(self.manager()),
                |enabled| {
                    called = true;
                    assert_eq!(enabled, expected_proxy);
                    assert!(!self.manager().get_backups()?.is_empty());
                    Ok(())
                },
                |_| panic!("unexpected Codex proxy change"),
            )?;
            assert!(called);
            Ok(())
        }
    }

    #[test]
    fn subscription_media_lives_in_the_1p_config_and_follows_the_mode() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let config_1p = fixture
            .root
            .path()
            .join("Claude/claude_desktop_config.json");
        std::fs::create_dir_all(config_1p.parent().context("1p configuration directory")?)?;
        let other = json!({"command": "other-mcp", "args": []});
        std::fs::write(
            &config_1p,
            json!({"preferences": {"sidebarMode": "a"}, "mcpServers": {"other": other}})
                .to_string(),
        )?;
        let registration = registration(fixture.root.path())?;
        let ours = json!({
            "command": registration.command,
            "args": [crate::media::stdio::ARG, registration.port.to_string(), registration.headers],
        });
        let prepare = |mode, media| {
            prepare_with_media(
                &fixture.dir,
                &config_1p,
                Some((mode, "token")),
                media,
                &registration,
                Some(&fixture_models()),
            )
        };
        let media_step = |plan: &PreparedPlan| {
            plan.steps().iter().find_map(|step| match step {
                PreparedStep::SetClaudeDesktop1pMedia { enabled, .. } => Some(*enabled),
                _ => None,
            })
        };
        let read_config_1p = || json::read(&config_1p);

        // No grant, no change: the 1p configuration is not touched.
        assert_eq!(media_step(&prepare(RoutingMode::Sub, false)?), None);

        let plan = prepare(RoutingMode::Sub, true)?;
        assert_eq!(media_step(&plan), Some(true));
        // Claude Desktop rewrites its 1p configuration while it runs.
        std::fs::write(
            &config_1p,
            json!({"preferences": {"sidebarMode": "b"}, "mcpServers": {
                "other": other,
                "coreinfra_media": {
                    "command": "previous-wizard",
                    "args": [crate::media::stdio::LEGACY_ARG, registration.port.to_string(), registration.headers],
                    "env": {}
                }
            }})
                .to_string(),
        )?;
        fixture.apply(plan, true)?;
        assert_eq!(
            read_config_1p()?["mcpServers"],
            json!({"other": other, "coreinfra_media": ours})
        );
        assert_eq!(read_config_1p()?["preferences"]["sidebarMode"], "b");
        assert_eq!(media_step(&prepare(RoutingMode::Sub, true)?), None);
        // The 1p configuration is Claude's own state, never part of a Wizard backup.
        assert!(!AgentKind::ClaudeDesktop.backs_up(&config_1p));

        // Hub keeps its registration in the profile instead.
        let plan = prepare(RoutingMode::Hub, true)?;
        assert_eq!(media_step(&plan), Some(false));
        fixture.apply(plan, true)?;
        assert_eq!(read_config_1p()?["mcpServers"], json!({"other": other}));
        assert_eq!(read_config_1p()?["preferences"]["sidebarMode"], "b");

        let stale = json!({
            "command": "previous-wizard",
            "args": [crate::media::stdio::LEGACY_ARG, registration.port.to_string(), registration.headers]
        });
        std::fs::write(
            &config_1p,
            json!({"mcpServers": {"coreinfra_media": stale}}).to_string(),
        )?;
        let plan = prepare(RoutingMode::Disabled, true)?;
        assert_eq!(media_step(&plan), Some(true));
        fixture.apply(plan, false)?;
        assert_eq!(
            read_config_1p()?,
            json!({"mcpServers": {"coreinfra_media": ours}})
        );
        assert_eq!(media_step(&prepare(RoutingMode::Disabled, true)?), None);
        fixture.apply(prepare(RoutingMode::Disabled, false)?, false)?;
        assert_eq!(read_config_1p()?, json!({}));

        std::fs::remove_file(&config_1p)?;
        assert_eq!(media_step(&prepare(RoutingMode::Hub, true)?), None);
        assert!(!config_1p.exists());
        Ok(())
    }

    #[test]
    fn hub_media_registration_follows_the_grant() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let config_1p = fixture
            .root
            .path()
            .join("Claude/claude_desktop_config.json");
        let registration = registration(fixture.root.path())?;
        let prepare = |grant| {
            prepare_with_media(
                &fixture.dir,
                &config_1p,
                Some((RoutingMode::Hub, "token")),
                grant,
                &registration,
                Some(&fixture_models()),
            )
        };
        let media_needs_apply = |grant| -> Result<bool> {
            Ok(
                !prepare_with_media(&fixture.dir, &config_1p, None, grant, &registration, None)?
                    .info()
                    .is_empty(),
            )
        };
        let example = json!({"name":"example", "transport":"http", "url":"https://mcp.example"});
        // (MCP servers named coreinfra_media, Wizard marketplaces) in the profile.
        let media_entries = || -> Result<(usize, usize)> {
            let profile = json::read(&fixture.files[0])?;
            let count = |key: &str, field: &str, value: &str| {
                profile[key].as_array().map_or(0, |entries| {
                    entries.iter().filter(|e| e[field] == value).count()
                })
            };
            assert_eq!(profile["managedMcpServers"][0], example);
            Ok((
                count("managedMcpServers", "name", "coreinfra_media"),
                count(
                    "allowedPluginMarketplaces",
                    "expectedName",
                    "coreinfra-wizard",
                ),
            ))
        };

        // Hub without a grant registers nothing, and detection agrees.
        fixture.apply(prepare(false)?, true)?;
        assert_eq!(media_entries()?, (0, 0));
        assert!(!media_needs_apply(false)?);
        assert!(media_needs_apply(true)?);

        fixture.apply(prepare(true)?, true)?;
        assert_eq!(media_entries()?, (1, 1));
        assert!(!media_needs_apply(true)?);
        assert!(media_needs_apply(false)?);

        let current = json::read(&fixture.files[0])?;
        let mut stale = current.clone();
        stale["managedMcpServers"][1]["args"][2] = json!(
            registration
                .headers
                .with_file_name("media-mcp-headers.json")
        );
        std::fs::write(&fixture.files[0], stale.to_string())?;
        assert!(media_needs_apply(true)?);
        fixture.apply(prepare(true)?, true)?;
        assert_eq!(json::read(&fixture.files[0])?, current);

        let mut duplicate = current["managedMcpServers"][1].clone();
        duplicate["args"][0] = json!(crate::media::stdio::LEGACY_ARG);
        stale["managedMcpServers"] = json!([duplicate, example, stale["managedMcpServers"][1]]);
        stale["allowedPluginMarketplaces"] = json!([
            {"repo":"coreinfraai/WIZARD", "path":"old"},
            {"expectedName":"coreinfra-wizard", "ref":"old"}
        ]);
        std::fs::write(&fixture.files[0], stale.to_string())?;
        fixture.apply(prepare(true)?, true)?;
        let repaired = json::read(&fixture.files[0])?;
        assert_eq!(
            repaired["managedMcpServers"],
            json!([current["managedMcpServers"][1], example])
        );
        assert_eq!(
            repaired["allowedPluginMarketplaces"],
            current["allowedPluginMarketplaces"]
        );
        assert!(!media_needs_apply(true)?);

        // Revoking the grant removes only Wizard's entries.
        std::fs::write(&fixture.files[0], stale.to_string())?;
        fixture.apply(prepare(false)?, true)?;
        assert_eq!(media_entries()?, (0, 0));

        // A foreign `coreinfra_media` server blocks enabling and is preserved when disabling.
        let foreign =
            json!({"name":"coreinfra_media", "transport":"http", "url":"https://other.example"});
        let mut with_foreign = json::read(&fixture.files[0])?;
        with_foreign["managedMcpServers"] = json!([foreign, example]);
        std::fs::write(&fixture.files[0], with_foreign.to_string())?;
        assert!(prepare(true).is_err());
        fixture.apply(prepare(false)?, true)?;
        assert_eq!(
            json::read(&fixture.files[0])?["managedMcpServers"],
            json!([foreign, example])
        );
        Ok(())
    }

    #[test]
    fn upgrade_from_1_0_10_replaces_only_wizard_video_registrations() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let config_1p = fixture
            .root
            .path()
            .join("Claude/claude_desktop_config.json");
        std::fs::create_dir_all(config_1p.parent().context("1p configuration directory")?)?;
        let registration = registration(fixture.root.path())?;
        let legacy_headers = registration
            .headers
            .with_file_name("video-claude-desktop-headers.json");
        let grant = r#"{"Authorization":"Bearer wizard-video-grant"}"#;
        std::fs::write(&legacy_headers, grant)?;
        let legacy = |command: &str| {
            json!({"command": command, "args": [
                crate::media::stdio::LEGACY_ARG, "33789", legacy_headers
            ]})
        };
        let foreign = json!({"command": "other-video-mcp", "args": ["--serve"]});
        let prepare = |mode, grant| {
            prepare_with_media(
                &fixture.dir,
                &config_1p,
                Some((mode, "token")),
                grant,
                &registration,
                Some(&fixture_models()),
            )
        };

        // Subscription: the 1p registration moves to the new name and headers file.
        std::fs::write(
            &config_1p,
            json!({"mcpServers": {"coreinfra_video": legacy("/Applications/Old Wizard")}})
                .to_string(),
        )?;
        let media_needs_apply = |grant| -> Result<bool> {
            Ok(
                !prepare_with_media(&fixture.dir, &config_1p, None, grant, &registration, None)?
                    .info()
                    .is_empty(),
            )
        };
        fixture.apply(prepare(RoutingMode::Sub, true)?, true)?;
        let servers = json::read(&config_1p)?["mcpServers"].clone();
        assert_eq!(servers.as_object().map(serde_json::Map::len), Some(1));
        assert_eq!(
            servers["coreinfra_media"]["args"][0],
            crate::media::stdio::ARG
        );
        assert_eq!(std::fs::read_to_string(&registration.headers)?, grant);
        assert!(!legacy_headers.exists());
        assert!(!media_needs_apply(true)?);

        // A foreign server under the old name survives enabling and disabling.
        std::fs::write(
            &config_1p,
            json!({"mcpServers": {"coreinfra_video": foreign}}).to_string(),
        )?;
        for grant in [true, false] {
            fixture.apply(prepare(RoutingMode::Sub, grant)?, true)?;
            assert_eq!(
                json::read(&config_1p)?["mcpServers"]["coreinfra_video"],
                foreign
            );
        }

        // Hub: the 3p profile drops the Wizard connector, preserves toolPolicy overrides, and allows catalogs.
        std::fs::write(
            &fixture.files[2],
            json!({"deploymentMode":"3p"}).to_string(),
        )?;
        let mut profile = json::read(&fixture.files[0])?;
        let mut stdio_legacy = legacy("/Applications/Old Wizard");
        stdio_legacy["name"] = json!("coreinfra_video");
        stdio_legacy["transport"] = json!("stdio");
        stdio_legacy["toolPolicy"] = json!({"video_create":"deny", "video_models":"deny"});
        let mut stdio_foreign = foreign.clone();
        stdio_foreign["name"] = json!("coreinfra_video");
        profile["managedMcpServers"] = json!([stdio_legacy, stdio_foreign]);
        std::fs::write(&fixture.files[0], profile.to_string())?;
        std::fs::write(&legacy_headers, grant)?;
        std::fs::remove_file(&registration.headers)?;
        assert!(media_needs_apply(true)?);
        fixture.apply(prepare(RoutingMode::Hub, true)?, true)?;
        let servers = json::read(&fixture.files[0])?["managedMcpServers"].clone();
        assert_eq!(servers[0]["name"], "coreinfra_media");
        assert_eq!(
            servers[0]["toolPolicy"],
            json!({"video_create":"deny", "video_models":"deny", "speech_models":"allow", "transcription_models":"allow", "image_models":"allow"})
        );
        assert_eq!(servers[1], stdio_foreign);
        assert_eq!(servers.as_array().map(Vec::len), Some(2));
        assert_eq!(std::fs::read_to_string(&registration.headers)?, grant);

        // Restoring the 1.0.10 backup reconciles the connector to the current headers path.
        let backup_id = u32::try_from(fixture.manager().get_backups()?.len())?;
        fixture.apply(
            prepare_restore_with_media(
                &fixture.dir,
                &config_1p,
                &fixture.manager(),
                backup_id,
                true,
                &registration,
            )?,
            true,
        )?;
        let restored = json::read(&fixture.files[0])?["managedMcpServers"].clone();
        assert_eq!(restored[0]["name"], "coreinfra_media");
        assert_eq!(restored[0]["args"][2], json!(registration.headers));

        // A foreign `coreinfra_media` entry in 1p blocks enabling and survives disabling.
        std::fs::write(
            &config_1p,
            json!({"mcpServers": {"coreinfra_media": foreign}}).to_string(),
        )?;
        assert!(prepare(RoutingMode::Sub, true).is_err());
        fixture.apply(prepare(RoutingMode::Sub, false)?, true)?;
        assert_eq!(
            json::read(&config_1p)?["mcpServers"]["coreinfra_media"],
            foreign
        );

        fixture.apply(prepare(RoutingMode::Hub, false)?, true)?;
        assert_eq!(
            json::read(&fixture.files[0])?["managedMcpServers"],
            json!([stdio_foreign])
        );
        Ok(())
    }

    #[test]
    fn media_only_follows_the_active_deployment_without_configuring_routing() -> Result<()> {
        for has_profile in [false, true] {
            let fixture = Fixture::new("bedrock")?;
            let registration = registration(fixture.root.path())?;
            if !has_profile {
                for path in &fixture.files {
                    std::fs::remove_file(path)?;
                }
            }
            let original = json::read(&fixture.files[0])?;
            let selection = fixture.snapshots()?;
            let config_1p = fixture
                .root
                .path()
                .join("Claude/claude_desktop_config.json");
            let prepare = |grant| {
                prepare_with_media(&fixture.dir, &config_1p, None, grant, &registration, None)
            };
            for enable in [true, false] {
                let plan = prepare(enable)?;
                assert!(!plan.info().is_empty());
                plan.apply_with_backup(
                    Some(fixture.manager()),
                    |_| panic!("a media-only change must preserve the Claude proxy"),
                    |_| panic!("unexpected Codex proxy change"),
                )?;
                assert!(prepare(enable)?.info().is_empty());
                assert_eq!(read_profile(&fixture.dir)?.0, RoutingMode::Disabled);
                assert!(fixture.snapshots()?[1..] == selection[1..]);
                let local = json::read(&config_1p)?;
                assert_eq!(
                    local["mcpServers"].get("coreinfra_media").is_some(),
                    enable && !has_profile
                );
                if has_profile && enable {
                    let profile = json::read(&fixture.files[0])?;
                    assert_eq!(profile["managedMcpServers"][1]["name"], "coreinfra_media");
                    assert_eq!(profile["inferenceProvider"], original["inferenceProvider"]);
                    assert_eq!(profile["egressProxyUrl"], original["egressProxyUrl"]);
                } else {
                    assert_eq!(json::read(&fixture.files[0])?, original);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn foreign_connections_are_replaced_and_full_configuration_can_be_restored() -> Result<()> {
        for provider in ["gateway", "bedrock", "vertex"] {
            for mode in [RoutingMode::Hub, RoutingMode::Sub, RoutingMode::Disabled] {
                let fixture = Fixture::new(provider)?;
                let original = fixture.snapshots()?;
                let plan = prepare_routing_at(&fixture.dir, mode, "new-token")?;
                assert!(fixture.snapshots()? == original);
                assert!(fixture.manager().get_backups()?.is_empty());
                fixture.apply(plan, mode != RoutingMode::Disabled)?;
                assert_eq!(fixture.manager().get_backups()?.len(), 1);
                let restored = fixture.manager().restore_changes(1)?;
                // Include unchanged selection and preferences, not just the changed profile.
                assert_eq!(restored.len(), 4);
                for (path, snapshot) in fixture.files.iter().zip(&original) {
                    assert!(
                        restored
                            .iter()
                            .any(|change| change.path() == path && change.after == *snapshot)
                    );
                }
                let config = json::read(&fixture.files[0])?;
                assert_eq!(
                    config["managedMcpServers"][0],
                    json!({"name":"example", "transport":"http", "url":"https://mcp.example"})
                );
                assert_eq!(config["autoModeEnabled"], true);
                assert!(config.get("inferenceCredentialCommand").is_none());
                assert!(config.get("inferenceBedrockRegion").is_none());
                assert!(config.get("egressProxyPacUrl").is_none());
                assert!(config.get("egressProxyAuth").is_none());
                assert_eq!(read_profile(&fixture.dir)?.0, mode);
                if mode == RoutingMode::Hub {
                    assert!(config.get("inferenceGatewayApiKey").is_none());
                    assert_eq!(config["inferenceGatewayBaseUrl"], HUB_URL);
                    assert_eq!(config["inferenceCredentialKind"], "helper-script");
                    assert_eq!(config["inferenceGatewayAuthScheme"], "x-api-key");
                    assert_eq!(config["inferenceModels"], fixture_models().value()?);
                    assert!(!read_profile(&fixture.dir)?.2);
                } else {
                    assert!(
                        config
                            .as_object()
                            .expect("test profile must be present")
                            .keys()
                            .all(|key| !key.starts_with("inference"))
                    );
                }
                if mode == RoutingMode::Disabled {
                    assert!(config.get("egressProxyUrl").is_none());
                } else {
                    assert_eq!(
                        config["egressProxyUrl"],
                        format!("http://{}", wizard_proxy::ADDRESS)
                    );
                }
                fixture.apply(
                    prepare_restore_at(&fixture.dir, &fixture.manager(), 1)?,
                    false,
                )?;
                assert!(fixture.snapshots()? == original);
            }
        }
        Ok(())
    }

    #[test]
    fn retired_proxy_configuration_is_repaired_on_apply_and_backup_restore() -> Result<()> {
        for mode in [RoutingMode::Hub, RoutingMode::Sub] {
            let fixture = Fixture::new("gateway")?;
            let token = if mode == RoutingMode::Hub {
                "api-token"
            } else {
                ""
            };
            fixture.apply(prepare_routing_at(&fixture.dir, mode, token)?, true)?;

            let mut profile = json::read(&fixture.files[0])?;
            profile["egressProxyUrl"] = json!(RETIRED_PROXY_URL);
            std::fs::write(&fixture.files[0], profile.to_string())?;
            let (detected, agent_token, needs_apply) = read_profile(&fixture.dir)?;
            assert_eq!(detected, mode);
            assert!(needs_apply);
            assert_eq!(agent_token.is_some(), mode == RoutingMode::Hub);
            // Detection leaves the user's profile unchanged; Apply confirms the repair.
            assert_eq!(json::read(&fixture.files[0])?, profile);

            fixture.apply(prepare_routing_at(&fixture.dir, mode, token)?, true)?;
            assert_eq!(fixture.manager().get_backups()?.len(), 2);
            assert!(!read_profile(&fixture.dir)?.2);

            // Restoring the backup of the retired configuration keeps the two-port layout.
            fixture.apply(
                prepare_restore_at(&fixture.dir, &fixture.manager(), 2)?,
                true,
            )?;
            assert_eq!(
                json::read(&fixture.files[0])?["egressProxyUrl"],
                "http://127.0.0.1:33787"
            );
            let (restored_mode, restored_token, needs_apply) = read_profile(&fixture.dir)?;
            assert_eq!(restored_mode, mode);
            assert!(!needs_apply);
            assert_eq!(
                restored_token.map(AgentToken::into_secret).as_deref(),
                (mode == RoutingMode::Hub).then_some("api-token")
            );
        }
        Ok(())
    }

    #[test]
    fn backup_failure_prevents_configuration_and_proxy_changes() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let original = fixture.snapshots()?;
        // A file where the backup directory should be makes saving impossible.
        std::fs::write(fixture.root.path().join("backups"), "blocked")?;
        let plan = prepare_routing_at(&fixture.dir, RoutingMode::Hub, "new-token")?;
        let result = plan.apply_with_backup(
            Some(fixture.manager()),
            |_| panic!("proxy must not change before the backup is saved"),
            |_| panic!("unexpected Codex proxy change"),
        );
        assert!(result.is_err());
        assert!(fixture.snapshots()? == original);
        Ok(())
    }

    #[test]
    fn new_profile_can_be_removed_by_restoring_its_backup() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        for path in &fixture.files {
            std::fs::remove_file(path)?;
        }
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
            true,
        )?;
        let meta = json::read(&fixture.files[1])?;
        let created = fixture.dir.join(format!(
            "{}.json",
            applied_id(&meta)?.expect("test profile must be present")
        ));
        assert!(created.exists());
        fixture.apply(
            prepare_restore_at(&fixture.dir, &fixture.manager(), 1)?,
            false,
        )?;
        assert!(!created.exists());
        assert!(fixture.files.iter().all(|path| !path.exists()));
        assert_eq!(read_profile(&fixture.dir)?.0, RoutingMode::Disabled);
        Ok(())
    }

    #[test]
    fn restoring_after_selecting_another_profile_keeps_a_restorable_backup() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let original = fixture.snapshots()?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
            true,
        )?;

        let second_id = uuid::Uuid::new_v4().to_string();
        let second_path = fixture.dir.join(format!("{second_id}.json"));
        let second_config = json!({"inferenceProvider": "bedrock"});
        std::fs::write(&second_path, second_config.to_string())?;
        let mut meta = json::read(&fixture.files[1])?;
        meta["appliedId"] = json!(second_id);
        meta["entries"]
            .as_array_mut()
            .expect("profile entries")
            .push(json!({"id": second_id, "name": "Second"}));
        std::fs::write(&fixture.files[1], meta.to_string())?;
        let before_restore = fixture.snapshots()?;

        fixture.apply(
            prepare_restore_at(&fixture.dir, &fixture.manager(), 1)?,
            false,
        )?;
        assert!(fixture.snapshots()? == original);
        assert_eq!(fixture.manager().get_backups()?.len(), 2);

        fixture.apply(
            prepare_restore_at(&fixture.dir, &fixture.manager(), 2)?,
            false,
        )?;
        assert!(fixture.snapshots()? == before_restore);
        assert_eq!(json::read(&second_path)?, second_config);
        Ok(())
    }

    #[test]
    fn restoring_recovers_corrupt_metadata_and_preserves_organization_selection() -> Result<()> {
        for current_meta in [
            "{broken",
            r#"{"hybridPointer":{"url":"https://org.example/config"}}"#,
        ] {
            let fixture = Fixture::new("gateway")?;
            let original = fixture.snapshots()?;
            fixture.apply(
                prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
                true,
            )?;
            std::fs::write(&fixture.files[1], current_meta)?;
            fixture.apply(
                prepare_restore_at(&fixture.dir, &fixture.manager(), 1)?,
                false,
            )?;
            assert!(fixture.snapshots()? == original);
            let saved = fixture.manager().restore_changes(2)?;
            assert!(saved.iter().any(|change| change.path() == &fixture.files[1]
                && change.after.content() == Some(current_meta)));
            if current_meta.starts_with("{\"hybridPointer\"") {
                fixture.apply(
                    prepare_restore_at(&fixture.dir, &fixture.manager(), 2)?,
                    false,
                )?;
                assert_eq!(std::fs::read_to_string(&fixture.files[1])?, current_meta);
            }
        }
        Ok(())
    }

    #[test]
    fn manual_edits_get_a_new_backup_and_restoring_local_proxy_enables_it() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
            true,
        )?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Sub, "")?,
            true,
        )?;
        assert_eq!(fixture.manager().get_backups()?.len(), 1);
        let mut config = json::read(&fixture.files[0])?;
        config["autoModeEnabled"] = json!(false);
        std::fs::write(&fixture.files[0], config.to_string())?;
        let edited = fixture.snapshots()?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Disabled, "")?,
            false,
        )?;
        assert_eq!(fixture.manager().get_backups()?.len(), 2);
        fixture.apply(
            prepare_restore_at(&fixture.dir, &fixture.manager(), 2)?,
            true,
        )?;
        assert!(fixture.snapshots()? == edited);
        Ok(())
    }

    #[test]
    fn invalid_catalog_or_old_default_requires_reapply() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
            true,
        )?;
        let baseline = json::read(&fixture.files[0])?;
        assert!(!read_profile(&fixture.dir)?.2);
        let models = &baseline["inferenceModels"];
        assert_eq!(
            models[0],
            json!({"name":"claude-coreinfra-cn-1", "labelOverride":"DeepSeek V4.1 Flash", "behavesAs":"claude-sonnet-4-6", "maxEffort":"max"})
        );
        assert_eq!(models[1]["name"], "claude-sonnet-5");
        assert_eq!(models[2]["name"], "claude-coreinfra-cn-3");
        let previous_order = json!([models[1], models[2], models[0]]);
        for models in [
            Value::Null,
            json!([]),
            json!(["invalid-model"]),
            previous_order,
        ] {
            let mut config = baseline.clone();
            config["inferenceModels"] = models;
            std::fs::write(&fixture.files[0], config.to_string())?;
            assert!(read_profile(&fixture.dir)?.2);
            fixture.apply(
                prepare_routing_at(&fixture.dir, RoutingMode::Hub, "token")?,
                true,
            )?;
            assert_eq!(json::read(&fixture.files[0])?, baseline);
            assert!(!read_profile(&fixture.dir)?.2);
        }
        Ok(())
    }

    #[test]
    fn catalog_refresh_preserves_mcp_and_a_missing_catalog_changes_no_files() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let config_1p = fixture
            .root
            .path()
            .join("Claude/claude_desktop_config.json");
        let registration = registration(&fixture.dir)?;
        let prepare = |models: Option<&Models>| {
            prepare_with_media(
                &fixture.dir,
                &config_1p,
                Some((RoutingMode::Hub, "selected-token")),
                true,
                &registration,
                models,
            )
        };
        let original = fixture.snapshots()?;
        assert!(prepare(None).is_err());
        assert!(fixture.snapshots()? == original);
        fixture.apply(prepare(Some(&fixture_models()))?, true)?;
        let before = json::read(&fixture.files[0])?;
        let updated_models = Models::from_value(&json!([
            {"name":"claude-coreinfra-cn-3", "labelOverride":"Updated GLM", "behavesAs":"claude-sonnet-4-6", "maxEffort":"high"}
        ])).expect("updated catalog");
        fixture.apply(prepare(Some(&updated_models))?, true)?;
        let after = json::read(&fixture.files[0])?;
        assert_eq!(after["inferenceModels"], updated_models.value()?);
        assert_eq!(
            read_profile(&fixture.dir)?
                .1
                .map(AgentToken::into_secret)
                .as_deref(),
            Some("selected-token")
        );
        assert_eq!(after["managedMcpServers"], before["managedMcpServers"]);
        assert_eq!(
            after["allowedPluginMarketplaces"],
            before["allowedPluginMarketplaces"]
        );
        assert_eq!(after["managedMcpServers"][1]["name"], "coreinfra_media");
        assert!(!read_profile(&fixture.dir)?.2);
        let repeated = prepare(Some(&updated_models))?;
        assert!(repeated.steps().iter().all(|step| match step {
            PreparedStep::ChangeFile { change } => !change.is_changed(),
            _ => true,
        }));
        Ok(())
    }

    #[test]
    fn sealed_profile_migrates_and_gateway_and_media_follow_the_same_key_after_reload() -> Result<()>
    {
        let fixture = Fixture::new("gateway")?;
        let meta = json::read(&fixture.files[1])?;
        let id = applied_id(&meta)?.context("missing fixture profile")?;
        let key_path = credential::path(&fixture.dir, id)?;
        let mut profile = json::read(&fixture.files[0])?;
        profile["inferenceGatewayBaseUrl"] = json!(HUB_URL);
        profile["inferenceCredentialKind"] = json!("static");
        profile["inferenceGatewayApiKey"] = json!("__sealed__:fixture");
        std::fs::write(&fixture.files[0], profile.to_string())?;
        let (mode, token, needs_apply) = read_profile(&fixture.dir)?;
        assert_eq!(mode, RoutingMode::Hub);
        assert!(token.is_none() && needs_apply);
        assert!(prepare_routing_at(&fixture.dir, mode, "__sealed__:fixture").is_err());

        let first = crate::media::fingerprint("first-key");
        let second = crate::media::fingerprint("second-key");
        fixture.apply(prepare_routing_at(&fixture.dir, mode, "first-key")?, true)?;
        let profile = json::read(&fixture.files[0])?;
        assert_eq!(
            profile["inferenceCredentialHelperArgs"],
            json!([credential::ARG, id, first])
        );
        assert!(!profile.to_string().contains("first-key"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&key_path)?.permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            credential::helper_token(&fixture.dir, id, &first)?.into_secret(),
            "first-key"
        );
        let (_, reloaded, needs_apply) = read_profile(&fixture.dir)?;
        assert_eq!(
            reloaded
                .context("missing reloaded media key")?
                .into_secret(),
            "first-key"
        );
        assert!(!needs_apply);
        assert!(credential::helper_token(&fixture.dir, "../another-profile", &first).is_err());

        fixture.apply(prepare_routing_at(&fixture.dir, mode, "second-key")?, true)?;
        assert!(credential::helper_token(&fixture.dir, id, &first).is_err());
        assert_eq!(
            credential::helper_token(&fixture.dir, id, &second)?.into_secret(),
            "second-key"
        );
        let mut manual = json::read(&fixture.files[0])?;
        manual["inferenceCredentialHelper"] = json!("different-helper");
        std::fs::write(&fixture.files[0], manual.to_string())?;
        assert!(read_profile(&fixture.dir)?.1.is_none());
        assert!(credential::helper_token(&fixture.dir, id, &second).is_err());
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Disabled, "")?,
            false,
        )?;
        assert!(!key_path.exists());
        Ok(())
    }

    #[test]
    fn credential_helper_handles_moved_wizard_windows_override_and_legacy_restore() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let meta = json::read(&fixture.files[1])?;
        let id = applied_id(&meta)?.context("missing fixture profile")?;
        let key_path = credential::path(&fixture.dir, id)?;
        let registration = tests::registration(&fixture.dir)?;
        fixture.apply(
            prepare_routing_at(&fixture.dir, RoutingMode::Hub, "first-key")?,
            true,
        )?;

        // Matching helper is healthy; moved helper keeps the token and flags reapply.
        let (mode, token, needs_apply) =
            read_profile_with_helper(&fixture.dir, Some(&registration.command))?;
        assert_eq!(mode, RoutingMode::Hub);
        assert_eq!(
            token.map(AgentToken::into_secret).as_deref(),
            Some("first-key")
        );
        assert!(!needs_apply);

        let moved = fixture.root.path().join("moved-wizard");
        let (mode, token, needs_apply) = read_profile_with_helper(&fixture.dir, Some(&moved))?;
        assert_eq!(mode, RoutingMode::Hub);
        assert_eq!(
            token.map(AgentToken::into_secret).as_deref(),
            Some("first-key")
        );
        assert!(needs_apply);
        assert!(!read_profile(&fixture.dir)?.2);

        // On Windows, a non-matching inferenceCredentialHelperWindows override rejects the sidecar.
        let base_profile = json::read(&fixture.files[0])?;
        for invalid_override in [json!("C:\\Other\\helper.exe"), json!(""), Value::Null] {
            let mut overridden = base_profile.clone();
            overridden["inferenceCredentialHelperWindows"] = invalid_override;
            let (win_token, win_needs_apply) =
                credential::profile_token_for_platform(&fixture.dir, id, &overridden, None, true)?;
            assert!(win_token.is_none() && win_needs_apply);
            let (unix_token, unix_needs_apply) =
                credential::profile_token_for_platform(&fixture.dir, id, &overridden, None, false)?;
            assert_eq!(
                unix_token.map(AgentToken::into_secret).as_deref(),
                Some("first-key")
            );
            assert!(!unix_needs_apply);
        }

        // Restoring a pre-1.0.12 first-launch backup (all missing, no `.key` entry) deletes `.key`.
        let backups_dir = fixture.root.path().join("backups");
        std::fs::create_dir_all(&backups_dir)?;
        std::fs::write(
            backups_dir.join("10.json"),
            serde_json::to_string_pretty(&json!({
                "id": 10,
                "version": 1,
                "time_created": "2026-01-01T00:00:00Z",
                "files": {
                    fixture.files[0].to_string_lossy().into_owned(): FileSnapshot::Missing,
                    fixture.files[1].to_string_lossy().into_owned(): FileSnapshot::Missing,
                    fixture.files[2].to_string_lossy().into_owned(): FileSnapshot::Missing,
                },
            }))?,
        )?;
        assert!(key_path.exists());
        fixture.apply(
            prepare_restore_at(&fixture.dir, &fixture.manager(), 10)?,
            false,
        )?;
        assert!(!fixture.files[0].exists());
        assert!(!fixture.files[1].exists());
        assert!(!key_path.exists());
        Ok(())
    }

    #[test]
    fn manually_configured_hub_supports_detection_and_mode_changes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let dir = directory.path().join("Claude-3p/configLibrary");
        std::fs::create_dir_all(&dir)?;
        let id = uuid::Uuid::new_v4().to_string();
        let meta = json!({"appliedId": id, "entries": [{"id": id, "name": "Manual"}]});
        std::fs::write(dir.join("_meta.json"), meta.to_string())?;
        let preferences_path = dir.with_file_name("claude_desktop_config.json");
        let profile_path = dir.join(format!("{id}.json"));

        for url in [
            HUB_URL,
            "https://hub.coreinfra.ai/claude/api",
            "https://hub.coreinfra.ai/anthropic/api",
        ] {
            for suffix in ["", "/"] {
                for mode in [RoutingMode::Hub, RoutingMode::Sub, RoutingMode::Disabled] {
                    let original = json!({
                        "inferenceProvider": "gateway",
                        "inferenceCredentialKind": "static",
                        "inferenceGatewayBaseUrl": format!("{url}{suffix}"),
                        "inferenceGatewayApiKey": "manual-token",
                        "inferenceGatewayAuthScheme": "x-api-key",
                        "managedMcpServers": [{"name":"example", "transport":"http", "url":"https://mcp.example"}],
                        "autoModeEnabled": true,
                    });
                    std::fs::write(&profile_path, original.to_string())?;
                    std::fs::write(&preferences_path, r#"{"deploymentMode":"3p"}"#)?;

                    let (detected, token, configuration_needs_apply) = read_profile(&dir)?;
                    assert_eq!(detected, RoutingMode::Hub);
                    assert!(configuration_needs_apply);
                    assert_eq!(
                        token.map(AgentToken::into_secret).as_deref(),
                        Some("manual-token")
                    );

                    let plan = prepare_routing_at(&dir, mode, "wizard-token")?;
                    assert_eq!(json::read(&profile_path)?, original);
                    let changes: Vec<_> = plan
                        .steps
                        .into_iter()
                        .filter_map(|step| match step {
                            PreparedStep::ChangeFile { change } => Some(change),
                            PreparedStep::SetClaudeProxyEnabled { enabled } => {
                                assert_eq!(enabled, mode != RoutingMode::Disabled);
                                None
                            }
                            _ => panic!("unexpected Claude Desktop routing step"),
                        })
                        .collect();
                    FileChange::apply_all(&changes)?;

                    let updated = json::read(&profile_path)?;
                    assert_eq!(
                        updated["managedMcpServers"][0],
                        original["managedMcpServers"][0]
                    );
                    assert_eq!(updated["autoModeEnabled"], true);
                    assert_eq!(json::read(&dir.join("_meta.json"))?, meta);
                    assert_eq!(read_profile(&dir)?.0, mode);
                    if mode == RoutingMode::Hub {
                        assert_eq!(updated["inferenceGatewayBaseUrl"], HUB_URL);
                        assert_eq!(
                            read_profile(&dir)?
                                .1
                                .map(AgentToken::into_secret)
                                .as_deref(),
                            Some("wizard-token")
                        );
                        assert_eq!(updated["inferenceModels"], fixture_models().value()?);
                        assert!(!read_profile(&dir)?.2);
                    } else {
                        assert!(
                            updated
                                .as_object()
                                .expect("test profile must be present")
                                .keys()
                                .all(|key| !key.starts_with("inference"))
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
