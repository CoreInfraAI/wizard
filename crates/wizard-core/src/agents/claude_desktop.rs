use super::{
    AgentDetection,
    detection::{self, AgentInfo, DesktopAgent},
};
use crate::{
    config_files::{changes::FileChange, json},
    prepared_plan::{PreparedAgent, PreparedPlan, PreparedStep},
    settings::Settings,
};
use anyhow::{Context as _, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Mutex};

const HUB_URL: &str = "https://hub.coreinfra.ai/anthropic/api";
const CONNECTION_KEYS: [&str; 5] = [
    "inferenceProvider",
    "inferenceCredentialKind",
    "inferenceGatewayBaseUrl",
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
];
pub(crate) static HUB_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ClaudeDesktop {
    #[serde(flatten)]
    pub info: AgentInfo,
    pub proxy_installed: bool,
}

pub(super) fn detect(settings: &Settings) -> AgentDetection<ClaudeDesktop> {
    let info = match detection::detect_desktop(
        DesktopAgent::Claude,
        settings.claude_desktop_path_last.as_deref(),
    ) {
        AgentDetection::Found(info) => info,
        AgentDetection::NotFound => return AgentDetection::NotFound,
        AgentDetection::Error(error) => return AgentDetection::Error(error),
    };
    let proxy_installed = match proxy_installed() {
        Ok(installed) => installed,
        Err(error) => return AgentDetection::Error(format!("{error:#}")),
    };
    AgentDetection::Found(ClaudeDesktop {
        info,
        proxy_installed,
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

fn proxy_installed() -> Result<bool> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
    let dir = config_library()?;
    let meta = json::read(&dir.join("_meta.json"))?;
    let Some(id) = applied_id(&meta)? else {
        return Ok(false);
    };
    let config = json::read(&dir.join(format!("{id}.json")))?;
    Ok(has_hub_connection(&config))
}

/// Prepare a local profile update without writing files or restarting Claude.
pub fn prepare_hub(install: bool, settings: &Settings) -> Result<PreparedPlan> {
    let _guard = PreparedAgent::ClaudeDesktop.lock()?;
    let dir = config_library()?;
    let mut meta = FileChange::read(dir.join("_meta.json"))?;
    let metadata = json::parse(&meta.after)?;
    let selected = applied_id(&metadata)?;
    if !install && selected.is_none() {
        return Ok(PreparedPlan::new(
            PreparedAgent::ClaudeDesktop,
            vec![PreparedStep::ChangeFile { change: meta }],
        ));
    }
    let id = selected.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let mut config = FileChange::read(dir.join(format!("{id}.json")))?;
    if selected.is_none() {
        ensure!(
            config.before().content().is_none(),
            "new Claude Desktop profile already exists; prepare again"
        );
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

    json::update(&mut config.after, |config| {
        let remove = !install && has_hub_connection(config);
        let object = config
            .as_object_mut()
            .context("Claude Desktop configuration must be an object")?;
        if install {
            object.insert("inferenceProvider".into(), json!("gateway"));
            object.insert("inferenceCredentialKind".into(), json!("static"));
            object.insert("inferenceGatewayBaseUrl".into(), json!(HUB_URL));
            object.insert(
                "inferenceGatewayApiKey".into(),
                json!(settings.coreinfra_token),
            );
            object.insert("inferenceGatewayAuthScheme".into(), json!("x-api-key"));
        } else if remove {
            for key in CONNECTION_KEYS {
                object.remove(key);
            }
        }
        Ok(())
    })?;

    // Publish a newly created profile before selecting it. Keep unchanged steps too.
    Ok(PreparedPlan::new(
        PreparedAgent::ClaudeDesktop,
        vec![
            PreparedStep::ChangeFile { change: config },
            PreparedStep::ChangeFile { change: meta },
        ],
    ))
}
