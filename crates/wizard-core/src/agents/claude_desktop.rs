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
pub(crate) static CONFIG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ClaudeDesktop {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub routing_mode: RoutingMode,
    pub token: Option<AgentToken>,
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

    let (routing_mode, token) = match read_routing() {
        Ok(routing) => routing,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(ClaudeDesktop {
        info,
        routing_mode,
        token,
    })
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

/// Reads the token owned by the active routing mode without exposing the full profile.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
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
fn read_routing() -> Result<(RoutingMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    read_routing_at(&config_library()?)
}

fn read_routing_at(dir: &Path) -> Result<(RoutingMode, Option<AgentToken>)> {
    let meta = json::read(&dir.join("_meta.json"))?;
    let Some(id) = applied_id(&meta)? else {
        return Ok((RoutingMode::Disabled, None));
    };

    let config = json::read(&dir.join(format!("{id}.json")))?;
    let preferences = json::read(&dir.with_file_name("claude_desktop_config.json"))?;
    let proxy_url = format!("http://{}", wizard_proxy::ADDRESS);

    if preferences["deploymentMode"] == "1p"
        && config["egressProxyUrl"] == proxy_url
        && config.get("egressProxyPacUrl").is_none()
    {
        Ok((RoutingMode::Sub, None))
    } else if preferences["deploymentMode"] != "1p" && has_hub_connection(&config) {
        let token = config["inferenceGatewayApiKey"]
            .as_str()
            .and_then(|token| AgentToken::new(token.to_owned()));
        Ok((RoutingMode::Hub, token))
    } else {
        Ok((RoutingMode::Disabled, None))
    }
}

/// Prepare a local profile update without writing files or restarting Claude. Hub writes `token`
/// to the profile; Hub and Sub enable local proxy demand; Disabled ignores it.
pub fn prepare_routing(mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    // Hub and Sub point Claude at the local proxy, which Linux cannot start yet.
    #[cfg(target_os = "linux")]
    ensure!(
        mode == RoutingMode::Disabled,
        "Claude Desktop routing requires the local proxy, which is not available on Linux"
    );
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    prepare_routing_at(&config_library()?, mode, token)
}

fn prepare_routing_at(dir: &Path, mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
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
    Ok(PreparedPlan::new(
        PreparedAgent::ClaudeDesktop,
        vec![
            PreparedStep::ChangeFile { change: config },
            PreparedStep::ChangeFile { change: meta },
            PreparedStep::ChangeFile {
                change: preferences,
            },
            PreparedStep::SetClaudeProxyEnabled {
                enabled: mode != RoutingMode::Disabled,
            },
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_files::changes::FileSnapshot;

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
                    "managedMcpServers": {"example": {"url": "https://mcp.example"}},
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
                    config["managedMcpServers"],
                    json!({"example": {"url": "https://mcp.example"}})
                );
                assert_eq!(config["autoModeEnabled"], true);
                assert!(config.get("inferenceCredentialCommand").is_none());
                assert!(config.get("inferenceBedrockRegion").is_none());
                assert!(config.get("egressProxyPacUrl").is_none());
                assert!(config.get("egressProxyAuth").is_none());
                assert_eq!(read_routing_at(&fixture.dir)?.0, mode);
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
        assert_eq!(read_routing_at(&fixture.dir)?.0, RoutingMode::Disabled);
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
                        "managedMcpServers": {"example": {"url": "https://mcp.example"}},
                        "autoModeEnabled": true,
                    });
                    std::fs::write(&profile_path, original.to_string())?;
                    std::fs::write(&preferences_path, r#"{"deploymentMode":"3p"}"#)?;

                    let (detected, token) = read_routing_at(&dir)?;
                    assert_eq!(detected, RoutingMode::Hub);
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
                    assert_eq!(updated["managedMcpServers"], original["managedMcpServers"]);
                    assert_eq!(updated["autoModeEnabled"], true);
                    assert_eq!(json::read(&dir.join("_meta.json"))?, meta);
                    assert_eq!(read_routing_at(&dir)?.0, mode);
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
