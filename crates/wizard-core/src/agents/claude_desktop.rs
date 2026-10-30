use super::{
    AgentDetection, AgentToken, RoutingMode,
    detection::{self, AgentInfo, DesktopAgent},
};
use crate::{
    config_files::{changes::FileChange, json},
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
};
use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Mutex};
use wizard_settings::Settings;

const HUB_URL: &str = "https://hub.coreinfra.ai/anthropic/api";
const CONNECTION_KEYS: [&str; 5] = [
    "inferenceProvider",
    "inferenceCredentialKind",
    "inferenceGatewayBaseUrl",
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
];
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

    let (routing_mode, token) = match read_routing(settings) {
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
            .is_some_and(|url| url.trim_end_matches('/') == HUB_URL)
}

/// Reads the token owned by the active routing mode without exposing the full profile.
pub fn configured_token(settings: &Settings) -> Result<Option<String>> {
    Ok(read_routing(settings)?.1.map(AgentToken::into_secret))
}

/// Hub reads the profile's key. Sub has none in the profile; its token is the proxy setting.
fn read_routing(settings: &Settings) -> Result<(RoutingMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    let dir = config_library()?;
    let meta = json::read(&dir.join("_meta.json"))?;
    let Some(id) = applied_id(&meta)? else {
        return Ok((RoutingMode::Disabled, None));
    };

    let config = json::read(&dir.join(format!("{id}.json")))?;
    let preferences = json::read(&dir.with_file_name("claude_desktop_config.json"))?;
    let proxy_url = format!("http://{}", wizard_proxy::CLAUDE_DESKTOP_ADDRESS);

    if preferences["deploymentMode"] == "1p"
        && config["egressProxyUrl"] == proxy_url
        && config.get("egressProxyPacUrl").is_none()
    {
        Ok((
            RoutingMode::Sub,
            settings
                .claude_proxy_enabled
                .clone()
                .and_then(AgentToken::new),
        ))
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
/// to the profile; Hub and Sub save it as the proxy setting; Disabled ignores it.
pub fn prepare_routing(mode: RoutingMode, token: &str) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    let dir = config_library()?;
    let mut meta = FileChange::read(dir.join("_meta.json"))?;
    let metadata = json::parse(&meta.after)?;
    let selected = applied_id(&metadata)?;

    let id = selected.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let mut config = FileChange::read(dir.join(format!("{id}.json")))?;
    let mut preferences = FileChange::read(dir.with_file_name("claude_desktop_config.json"))?;

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

    let proxy_url = format!("http://{}", wizard_proxy::CLAUDE_DESKTOP_ADDRESS);
    // Wizard points every profile it configures at its proxy.
    let managed = json::parse(&config.after)?["egressProxyUrl"] == proxy_url;
    json::update(&mut config.after, |config| {
        // Remove only CoreInfra's connection; another gateway belongs to the user and is not
        // backed up. Claude ignores inference settings in the 1p mode that Sub selects.
        let remove = mode != RoutingMode::Hub && has_hub_connection(config);

        let object = config
            .as_object_mut()
            .context("Claude Desktop configuration must be an object")?;

        if mode == RoutingMode::Hub {
            object.insert("inferenceProvider".into(), json!("gateway"));
            object.insert("inferenceCredentialKind".into(), json!("static"));
            object.insert("inferenceGatewayBaseUrl".into(), json!(HUB_URL));
            object.insert("inferenceGatewayApiKey".into(), json!(token));
            object.insert("inferenceGatewayAuthScheme".into(), json!("x-api-key"));
        } else if remove {
            for key in CONNECTION_KEYS {
                object.shift_remove(key);
            }
        }

        if mode == RoutingMode::Sub {
            object.insert("disableAutoUpdates".into(), json!(true));
        } else if managed {
            // Undo Sub's override only in a profile Wizard configured.
            object.shift_remove("disableAutoUpdates");
        }

        if mode != RoutingMode::Disabled {
            object.insert("egressProxyUrl".into(), json!(proxy_url));
            object.shift_remove("egressProxyPacUrl");
        } else if managed {
            object.shift_remove("egressProxyUrl");
        }
        Ok(())
    })?;

    // Disabled returns a Wizard profile to the Claude account (1p), where sign-in works.
    if mode != RoutingMode::Disabled || managed {
        json::update(&mut preferences.after, |preferences| {
            preferences["deploymentMode"] =
                json!(if mode == RoutingMode::Hub { "3p" } else { "1p" });
            Ok(())
        })?;
    }

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
                token: (mode != RoutingMode::Disabled).then(|| token.to_owned()),
            },
        ],
    ))
}
