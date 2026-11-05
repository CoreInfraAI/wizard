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
            .is_some_and(|url| url.trim_end_matches('/') == HUB_URL)
}

/// Profiles are not backed up, so a mode must not replace the user's own proxy or connection.
fn ensure_no_foreign_settings(config: &Value, mode: RoutingMode, proxy_url: &str) -> Result<()> {
    if mode == RoutingMode::Disabled {
        return Ok(());
    }
    let set = |key: &str| config.get(key).is_some_and(|value| !value.is_null());
    ensure!(
        !set("egressProxyPacUrl")
            && (!set("egressProxyUrl") || config["egressProxyUrl"] == proxy_url),
        "Remove the egress proxy from the selected Claude Desktop profile or select another profile: CoreInfra routing would replace it"
    );
    // Claude selects its own account with the "anthropic" provider, which Hub may replace.
    let own_connection = !has_hub_connection(config)
        && CONNECTION_KEYS
            .iter()
            .any(|key| set(key) && !(*key == "inferenceProvider" && config[key] == "anthropic"));
    ensure!(
        mode != RoutingMode::Hub || !own_connection,
        "Remove the inference provider from the selected Claude Desktop profile or select another profile: CoreInfra Hub would replace it"
    );
    Ok(())
}

/// Reads the token owned by the active routing mode without exposing the full profile.
pub fn configured_token() -> Result<Option<String>> {
    Ok(read_routing()?.1.map(AgentToken::into_secret))
}

/// Hub reads the profile's key. Sub has no agent key; Wizard owns proxy credentials.
fn read_routing() -> Result<(RoutingMode, Option<AgentToken>)> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;

    let dir = config_library()?;
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

    let dir = config_library()?;
    let mut meta = FileChange::read(dir.join("_meta.json"))?;
    let metadata = json::parse(&meta.after)?;
    let selected = applied_id(&metadata)?;

    let id = selected.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let mut config = FileChange::read(dir.join(format!("{id}.json")))?;
    let mut preferences = FileChange::read(dir.with_file_name("claude_desktop_config.json"))?;

    let proxy_url = format!("http://{}", wizard_proxy::ADDRESS);
    let current = json::parse(&config.after)?;
    ensure_no_foreign_settings(&current, mode, &proxy_url)?;
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

    // Wizard points every profile it configures at its proxy.
    let managed = current["egressProxyUrl"] == proxy_url;
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
                enabled: mode != RoutingMode::Disabled,
            },
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_proxy_and_connection_are_not_replaced() {
        let proxy = "http://127.0.0.1:33787";
        let check = |config: Value, mode| ensure_no_foreign_settings(&config, mode, proxy);
        let modes = [RoutingMode::Hub, RoutingMode::Sub];

        for mode in modes {
            assert!(check(json!({}), mode).is_ok());
            assert!(check(json!({"egressProxyUrl": proxy}), mode).is_ok());
            assert!(check(json!({"egressProxyUrl": "http://corp:3128"}), mode).is_err());
            assert!(check(json!({"egressProxyPacUrl": "http://corp/pac"}), mode).is_err());
            // Claude's own account and CoreInfra's gateway may be replaced.
            assert!(check(json!({"inferenceProvider": "anthropic"}), mode).is_ok());
            let hub = json!({"inferenceProvider": "gateway", "inferenceGatewayBaseUrl": HUB_URL});
            assert!(check(hub, mode).is_ok());
        }
        let gateway = json!({"inferenceProvider": "gateway", "inferenceGatewayApiKey": "user"});
        assert!(check(gateway.clone(), RoutingMode::Hub).is_err());
        // Sub leaves inference settings unchanged.
        assert!(check(gateway, RoutingMode::Sub).is_ok());
        assert!(check(json!({"inferenceProvider": "bedrock"}), RoutingMode::Hub).is_err());
        let corp = json!({"egressProxyUrl": "http://corp:3128"});
        assert!(check(corp, RoutingMode::Disabled).is_ok());
    }
}
