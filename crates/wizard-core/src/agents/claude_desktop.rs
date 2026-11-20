mod video;

use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, AgentInfo, DesktopAgent},
};
use crate::{
    config_files::{
        backups::{AgentKind, BackupManager},
        changes::FileChange,
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

const HUB_URL: &str = "https://hub.coreinfra.ai/anthropic/api";
// Read-only compatibility with an unreleased configuration. Never bind this port.
const RETIRED_PROXY_URL: &str = "http://127.0.0.1:33789";
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ClaudeDesktop {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
    pub proxy_needs_apply: bool,
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

    let video = settings.video.claude_desktop.is_some();
    let (routing_mode, token, proxy_needs_apply) = match read_routing(video) {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(ClaudeDesktop {
        info,
        routing_mode,
        token,
        proxy_needs_apply,
    })
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
/// Only the Wizard-owned `coreinfra_video` entry changes.
pub(crate) fn prepare_1p_video(path: PathBuf, enable: bool) -> Result<FileChange> {
    video_1p_change(path, enable, &video::Registration::for_wizard()?)
}

fn video_1p_change(
    path: PathBuf,
    enable: bool,
    registration: &video::Registration,
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
                    HUB_URL | "https://hub.coreinfra.ai/claude/api"
                )
            })
}

pub fn get_backups() -> Result<Vec<super::Backup>> {
    BackupManager::new(AgentKind::ClaudeDesktop)?.get_backups()
}

pub fn prepare_restore_backup(id: u32) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
    prepare_restore_at(
        &config_library()?,
        &BackupManager::new(AgentKind::ClaudeDesktop)?,
        id,
    )
}

fn prepare_restore_at(dir: &Path, manager: &BackupManager, id: u32) -> Result<PreparedPlan> {
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
    // A restore also backs up the state it replaces. If the user has selected another
    // profile since this backup, include that profile so the new backup is restorable.
    let meta_path = dir.join("_meta.json");
    let current_meta = changes
        .iter()
        .find(|change| change.path() == &meta_path)
        .context("Claude Desktop backup is missing profile metadata")?;
    // Corrupt current metadata must not prevent recovery from a valid backup.
    if let Ok(current_meta) = json::parse(current_meta.before())
        && let Ok(Some(current_id)) = applied_id(&current_meta)
    {
        let current_path = dir.join(format!("{current_id}.json"));
        if !changes.iter().any(|change| change.path() == &current_path) {
            changes.push(FileChange::read(current_path)?);
        }
    }
    let restored = |path: &Path| -> Result<Value> {
        let change = changes
            .iter()
            .find(|change| change.path() == path)
            .context("Claude Desktop backup is missing a configuration file")?;
        json::parse(&change.after)
    };
    let meta = restored(&dir.join("_meta.json"))?;
    let proxy_enabled = if meta.get("hybridPointer").is_none_or(Value::is_null)
        && let Some(id) = applied_id(&meta)?
    {
        let profile = restored(&dir.join(format!("{id}.json")))?;
        profile["egressProxyUrl"] == format!("http://{}", wizard_proxy::ADDRESS)
            && profile.get("egressProxyPacUrl").is_none_or(Value::is_null)
    } else {
        false
    };
    let mut steps: Vec<_> = changes
        .into_iter()
        .map(|change| PreparedStep::ChangeFile { change })
        .collect();
    steps.push(PreparedStep::SetClaudeProxyEnabled {
        enabled: proxy_enabled,
    });
    Ok(PreparedPlan::new(PreparedAgent::ClaudeDesktop, steps))
}

/// Hub reads the profile's key. Sub has no agent key; Wizard owns proxy credentials.
/// `video`: whether Wizard holds a video grant for Claude Desktop.
fn read_routing(video: bool) -> Result<(RoutingMode, Option<AgentToken>, bool)> {
    let (mode, token, needs_apply) = read_profile(&config_library()?, Some(video))?;
    let config_1p_needs_apply =
        prepare_1p_video(config_1p_path()?, video && mode == RoutingMode::Sub)
            .map_or(true, |change| change.is_changed());
    Ok((mode, token, needs_apply || config_1p_needs_apply))
}

/// The key for paid video calls: the profile's key in Hub; in Sub, which has no agent key,
/// Wizard's own proxy key from its personal Hub. Never log it.
pub(crate) fn video_token() -> Result<Option<String>> {
    let (mode, token, _) = read_profile(&config_library()?, None)?;
    Ok(match mode {
        RoutingMode::Hub => token.map(AgentToken::into_secret),
        RoutingMode::Sub => wizard_settings::load_from_file()?
            .proxy_token
            .filter(|token| !token.is_empty()),
        RoutingMode::Disabled => None,
    })
}

/// `video_grant`: `Some` also checks the Hub video registration against the grant.
fn read_profile(
    dir: &Path,
    video_grant: Option<bool>,
) -> Result<(RoutingMode, Option<AgentToken>, bool)> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
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
        let token = config["inferenceGatewayApiKey"]
            .as_str()
            .and_then(|s| AgentToken::new(s.to_owned()));
        let Some(video_grant) = video_grant else {
            return Ok((RoutingMode::Hub, token, proxy_needs_apply));
        };
        let mut updated = config.clone();
        let registration = video::Registration::for_wizard()?;
        let needs_video = registration
            .update_3p_profile(
                updated
                    .as_object_mut()
                    .context("Invalid Claude Desktop profile")?,
                video_grant,
            )
            .is_err()
            || updated != config;
        Ok((RoutingMode::Hub, token, proxy_needs_apply || needs_video))
    } else {
        Ok((RoutingMode::Disabled, None, false))
    }
}

/// Prepare a local profile update without writing files or restarting Claude. Hub writes `token`
/// to the profile; Hub and Sub enable local proxy demand; Disabled ignores it.
/// `video`: whether Wizard holds a video grant for Claude Desktop.
pub fn prepare_routing(mode: RoutingMode, token: &str, video: bool) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    prepare_with_video(
        &config_library()?,
        &config_1p_path()?,
        mode,
        token,
        video,
        &video::Registration::for_wizard()?,
    )
}

#[cfg(test)]
fn prepare_routing_at(dir: &Path, mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    let config_1p = dir
        .parent()
        .and_then(Path::parent)
        .context("Missing test profile directory")?
        .join("Claude/claude_desktop_config.json");
    prepare_with_video(
        dir,
        &config_1p,
        mode,
        token,
        false,
        &video::Registration::for_wizard()?,
    )
}

fn prepare_with_video(
    dir: &Path,
    config_1p: &Path,
    mode: RoutingMode,
    token: &str,
    video_grant: bool,
    video: &video::Registration,
) -> Result<PreparedPlan> {
    let mut meta = FileChange::read(dir.join("_meta.json"))?;
    let metadata = json::parse(&meta.after)?;
    let selected = applied_id(&metadata)?;

    let id = selected.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let mut config = FileChange::read(dir.join(format!("{id}.json")))?;
    let mut preferences = FileChange::read(dir.with_file_name("claude_desktop_config.json"))?;

    let proxy_url = format!("http://{}", wizard_proxy::ADDRESS);
    ensure!(
        selected.is_some() || config.before().content().is_none(),
        "new Claude Desktop profile already exists; prepare again"
    );

    if selected.is_none() && mode != RoutingMode::Disabled {
        json::update(&mut meta.after, |meta| {
            let entries = meta
                .as_object_mut()
                .context("Claude Desktop metadata must be an object")?
                .entry("entries")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .context("Claude Desktop profile entries must be an array")?;
            ensure!(
                !entries.iter().any(|entry| entry["id"] == id),
                "new Claude Desktop profile identifier already exists; prepare again"
            );

            entries.push(json!({"id": id, "name": "CoreInfra"}));
            meta["appliedId"] = json!(id);
            Ok(())
        })?;
    }

    if selected.is_some() || mode != RoutingMode::Disabled {
        json::update(&mut config.after, |config| {
            let object = config
                .as_object_mut()
                .context("Claude Desktop configuration must be an object")?;

            video.update_3p_profile(object, video_grant && mode == RoutingMode::Hub)?;
            // The complete previous profile is backed up before applying this plan.
            // Clear provider-specific credentials and proxy overrides before setting the mode.
            object
                .retain(|key, _| !key.starts_with("inference") && !key.starts_with("egressProxy"));
            if mode == RoutingMode::Hub {
                object.insert("inferenceProvider".into(), json!("gateway"));
                object.insert("inferenceCredentialKind".into(), json!("static"));
                object.insert("inferenceGatewayBaseUrl".into(), json!(HUB_URL));
                object.insert("inferenceGatewayApiKey".into(), json!(token));
                object.insert("inferenceGatewayAuthScheme".into(), json!("x-api-key"));
            }

            if mode == RoutingMode::Sub {
                object.insert("disableAutoUpdates".into(), json!(true));
            } else {
                object.shift_remove("disableAutoUpdates");
            }

            if mode != RoutingMode::Disabled {
                object.insert("egressProxyUrl".into(), json!(proxy_url));
            }
            Ok(())
        })?;
    }

    // Disabled returns to the Claude account (1p), where sign-in works.
    json::update(&mut preferences.after, |preferences| {
        preferences["deploymentMode"] = json!(if mode == RoutingMode::Hub { "3p" } else { "1p" });
        Ok(())
    })?;

    // Publish a newly created profile before selecting it. Keep unchanged steps too.
    let mut steps = vec![
        PreparedStep::ChangeFile { change: config },
        PreparedStep::ChangeFile { change: meta },
        PreparedStep::ChangeFile {
            change: preferences,
        },
    ];
    // Claude Desktop rewrites its 1p configuration while running: read it again after
    // closing the app. Preparing here only rejects a foreign entry and skips a no-op.
    let enabled = video_grant && mode == RoutingMode::Sub;
    if video_1p_change(config_1p.to_owned(), enabled, video)?.is_changed() {
        steps.push(PreparedStep::SetClaudeDesktop1pVideo {
            path: config_1p.to_owned(),
            enabled,
        });
    }
    steps.push(PreparedStep::SetClaudeProxyEnabled {
        enabled: mode != RoutingMode::Disabled,
    });
    Ok(PreparedPlan::new(PreparedAgent::ClaudeDesktop, steps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_files::changes::FileSnapshot;

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
                    }
                    activated = true;
                    Ok(())
                },
                |_| panic!("unexpected ChatGPT proxy"),
            )?;
            assert!(activated);
            let (detected, token, needs_apply) = read_profile(&dir, Some(false))?;
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
    fn subscription_video_lives_in_the_1p_config_and_follows_the_mode() -> Result<()> {
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
        let registration = video::Registration::for_wizard()?;
        let ours = json!({
            "command": registration.command,
            "args": [crate::video::stdio::ARG, registration.port.to_string(), registration.headers],
        });
        let prepare = |mode, video| {
            prepare_with_video(
                &fixture.dir,
                &config_1p,
                mode,
                "token",
                video,
                &registration,
            )
        };
        let video_step = |plan: &PreparedPlan| {
            plan.steps().iter().find_map(|step| match step {
                PreparedStep::SetClaudeDesktop1pVideo { enabled, .. } => Some(*enabled),
                _ => None,
            })
        };
        let read_config_1p = || json::read(&config_1p);

        // No grant, no change: the 1p configuration is not touched.
        assert_eq!(video_step(&prepare(RoutingMode::Sub, false)?), None);

        let plan = prepare(RoutingMode::Sub, true)?;
        assert_eq!(video_step(&plan), Some(true));
        // Claude Desktop rewrites its 1p configuration while it runs.
        std::fs::write(
            &config_1p,
            json!({"preferences": {"sidebarMode": "b"}, "mcpServers": {"other": other}})
                .to_string(),
        )?;
        fixture.apply(plan, true)?;
        assert_eq!(
            read_config_1p()?["mcpServers"],
            json!({"other": other, "coreinfra_video": ours})
        );
        assert_eq!(read_config_1p()?["preferences"]["sidebarMode"], "b");
        assert_eq!(video_step(&prepare(RoutingMode::Sub, true)?), None);
        // The 1p configuration is Claude's own state, never part of a Wizard backup.
        assert!(!AgentKind::ClaudeDesktop.backs_up(&config_1p));

        // Hub keeps its registration in the profile instead.
        let plan = prepare(RoutingMode::Hub, true)?;
        assert_eq!(video_step(&plan), Some(false));
        fixture.apply(plan, true)?;
        assert_eq!(read_config_1p()?["mcpServers"], json!({"other": other}));
        assert_eq!(read_config_1p()?["preferences"]["sidebarMode"], "b");

        let foreign = json!({"command": "user-video", "args": []});
        std::fs::write(
            &config_1p,
            json!({"mcpServers": {"coreinfra_video": foreign}}).to_string(),
        )?;
        assert!(prepare(RoutingMode::Sub, true).is_err());
        assert_eq!(video_step(&prepare(RoutingMode::Disabled, false)?), None);
        assert_eq!(read_config_1p()?["mcpServers"]["coreinfra_video"], foreign);

        std::fs::remove_file(&config_1p)?;
        assert_eq!(video_step(&prepare(RoutingMode::Hub, true)?), None);
        assert!(!config_1p.exists());
        Ok(())
    }

    #[test]
    fn hub_video_registration_follows_the_grant() -> Result<()> {
        let fixture = Fixture::new("gateway")?;
        let config_1p = fixture
            .root
            .path()
            .join("Claude/claude_desktop_config.json");
        let registration = video::Registration::for_wizard()?;
        let prepare = |grant| {
            prepare_with_video(
                &fixture.dir,
                &config_1p,
                RoutingMode::Hub,
                "token",
                grant,
                &registration,
            )
        };
        let example = json!({"name":"example", "transport":"http", "url":"https://mcp.example"});
        // (MCP servers named coreinfra_video, Wizard marketplaces) in the profile.
        let video_entries = || -> Result<(usize, usize)> {
            let profile = json::read(&fixture.files[0])?;
            let count = |key: &str, field: &str, value: &str| {
                profile[key].as_array().map_or(0, |entries| {
                    entries.iter().filter(|e| e[field] == value).count()
                })
            };
            assert_eq!(profile["managedMcpServers"][0], example);
            Ok((
                count("managedMcpServers", "name", "coreinfra_video"),
                count(
                    "allowedPluginMarketplaces",
                    "expectedName",
                    "coreinfra-wizard",
                ),
            ))
        };

        // Hub without a grant registers nothing, and detection agrees.
        fixture.apply(prepare(false)?, true)?;
        assert_eq!(video_entries()?, (0, 0));
        assert!(!read_profile(&fixture.dir, Some(false))?.2);
        assert!(read_profile(&fixture.dir, Some(true))?.2);

        fixture.apply(prepare(true)?, true)?;
        assert_eq!(video_entries()?, (1, 1));
        assert!(!read_profile(&fixture.dir, Some(true))?.2);
        assert!(read_profile(&fixture.dir, Some(false))?.2);

        // Revoking the grant removes only Wizard's entries.
        fixture.apply(prepare(false)?, true)?;
        assert_eq!(video_entries()?, (0, 0));
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
                assert_eq!(restored.len(), 3);
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
                assert_eq!(read_profile(&fixture.dir, Some(false))?.0, mode);
                if mode == RoutingMode::Hub {
                    assert_eq!(config["inferenceGatewayApiKey"], "new-token");
                    assert_eq!(config["inferenceGatewayBaseUrl"], HUB_URL);
                    assert_eq!(config["inferenceCredentialKind"], "static");
                    assert_eq!(config["inferenceGatewayAuthScheme"], "x-api-key");
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
            let (detected, agent_token, needs_apply) = read_profile(&fixture.dir, Some(false))?;
            assert_eq!(detected, mode);
            assert!(needs_apply);
            assert_eq!(agent_token.is_some(), mode == RoutingMode::Hub);
            // Detection leaves the user's profile unchanged; Apply confirms the repair.
            assert_eq!(json::read(&fixture.files[0])?, profile);

            fixture.apply(prepare_routing_at(&fixture.dir, mode, token)?, true)?;
            assert_eq!(fixture.manager().get_backups()?.len(), 2);
            assert!(!read_profile(&fixture.dir, Some(false))?.2);

            // Restoring the backup of the retired configuration keeps the two-port layout.
            fixture.apply(
                prepare_restore_at(&fixture.dir, &fixture.manager(), 2)?,
                true,
            )?;
            assert_eq!(
                json::read(&fixture.files[0])?["egressProxyUrl"],
                "http://127.0.0.1:33787"
            );
            let (restored_mode, restored_token, needs_apply) =
                read_profile(&fixture.dir, Some(false))?;
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
        assert_eq!(
            read_profile(&fixture.dir, Some(false))?.0,
            RoutingMode::Disabled
        );
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
    fn manually_configured_hub_supports_detection_and_mode_changes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let dir = directory.path().join("configLibrary");
        std::fs::create_dir(&dir)?;
        let id = uuid::Uuid::new_v4().to_string();
        let meta = json!({"appliedId": id, "entries": [{"id": id, "name": "Manual"}]});
        std::fs::write(dir.join("_meta.json"), meta.to_string())?;
        let preferences_path = dir.with_file_name("claude_desktop_config.json");
        let profile_path = dir.join(format!("{id}.json"));

        for url in [HUB_URL, "https://hub.coreinfra.ai/claude/api"] {
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

                    // With a video grant, a manual Hub profile also lacks Wizard's registration.
                    let (detected, token, proxy_needs_apply) = read_profile(&dir, Some(true))?;
                    assert_eq!(detected, RoutingMode::Hub);
                    assert!(proxy_needs_apply);
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
                    assert_eq!(read_profile(&dir, Some(false))?.0, mode);
                    if mode == RoutingMode::Hub {
                        assert_eq!(updated["inferenceGatewayBaseUrl"], HUB_URL);
                        assert_eq!(updated["inferenceGatewayApiKey"], "wizard-token");
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
