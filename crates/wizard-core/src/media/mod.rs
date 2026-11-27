//! Stateless media tools. Video job references are carried by the caller and contain no
//! credential. Audio is saved or returned directly; Wizard never reads files for the caller.
mod audio;
pub mod background;
pub mod install;
pub mod keys;
pub mod server;
pub mod stdio;

use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
use reqwest::{Client, RequestBuilder, Response, Url};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt as _;

pub const ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 33789);
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub const URL: &str = "http://127.0.0.1:33789/mcp";
const OPENROUTER: &str = "/openrouter/api/v1/videos";
const FAL: &str = "/fal/api";
const OUTPUT_DIR: &str = "CoreInfra Media";
/// Responses read into memory: catalogs, job states and transcripts.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[must_use]
pub fn fingerprint(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}

#[derive(Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Provider {
    OpenRouter,
    Fal,
}

#[derive(Serialize)]
struct Model {
    id: String,
    provider: Provider,
    name: Option<String>,
    min_rub_per_second: Option<f64>,
    max_rub_per_second: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Create {
    model: String,
    /// Video description, when the selected model requires one.
    prompt: Option<String>,
    /// Extra top-level fields of the provider's JSON body, including inline image inputs.
    fields: Option<Map<String, Value>>,
    /// Alternative for clients that require fixed object schemas. A JSON object string.
    fields_json: Option<String>,
}

impl Create {
    fn into_body(self) -> Result<(String, Map<String, Value>)> {
        let mut body = extra_fields(self.fields, self.fields_json, &["model", "prompt"])?;
        ensure!(
            self.prompt.as_ref().is_some_and(|p| !p.trim().is_empty()) || !body.is_empty(),
            "Provide a prompt or model-specific inputs"
        );
        if let Some(prompt) = self.prompt {
            body.insert("prompt".into(), prompt.into());
        }
        Ok((self.model, body))
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    provider: Provider,
    model: String,
    id: String,
    pub key_id: String,
}

const MISSING_KEY: &str = "В активной конфигурации агента нет ключа CoreInfra. Проверьте настройки подключения агента в Wizard";

/// Merge `fields` or `fields_json` into a body after rejecting keys owned by named arguments
/// or nested wrapper fields.
fn extra_fields(
    fields: Option<Map<String, Value>>,
    fields_json: Option<String>,
    named: &[&str],
) -> Result<Map<String, Value>> {
    let body = match (fields, fields_json) {
        (Some(_), Some(_)) => bail!("Use either fields or fields_json"),
        (Some(fields), None) => fields,
        (None, Some(text)) => serde_json::from_str(&text)
            .map_err(|_| anyhow!("fields_json must encode a JSON object"))?,
        (None, None) => Map::new(),
    };
    if let Some(key) = named.iter().find(|key| body.contains_key(**key)) {
        bail!("Put {key} in the named tool argument, not in fields");
    }
    ensure!(
        !body.contains_key("fields") && !body.contains_key("fields_json"),
        "Put model options directly in fields or fields_json, not in a nested fields or fields_json"
    );
    Ok(body)
}

/// Cached for one agent. Never serialized or logged.
pub struct MediaKey {
    pub token: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetResult {
    /// Copy the entire job object returned by `video_create`.
    job: Job,
    /// Save a completed MP4 to the user's Downloads folder. Defaults to true.
    download: Option<bool>,
    /// Zero-based `OpenRouter` output index. fal has one output.
    #[serde(default)]
    index: u32,
}

#[derive(Deserialize)]
struct PriceEntry {
    category: String,
    endpoints: Vec<String>,
    display_name: Option<String>,
    #[serde(default)]
    prices: BTreeMap<String, Value>,
    #[serde(default)]
    lowest_prices: BTreeMap<String, Value>,
}

impl PriceEntry {
    /// Lowest and highest public rate for `unit`, scaled from Hub's RUB per million units.
    fn range(&self, unit: &str, scale: f64) -> (Option<f64>, Option<f64>) {
        let rate = |prices: &BTreeMap<String, Value>| {
            prices.get(unit).and_then(Value::as_f64).map(|p| p * scale)
        };
        let max = rate(&self.prices);
        (rate(&self.lowest_prices).or(max), max)
    }
}

#[derive(Clone)]
pub struct MediaHub {
    client: Client,
    origin: Url,
    output_dir: Option<PathBuf>,
}

impl MediaHub {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(wizard_proxy::user_agent())
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(180))
                .build()
                .map_err(|_| anyhow!("Could not initialize the media HTTP client"))?,
            origin: Url::parse(crate::hub::HUB_ORIGIN)?,
            output_dir: dirs::download_dir()
                .or_else(|| dirs::home_dir().map(|dir| dir.join("Downloads")))
                .map(|dir| dir.join(OUTPUT_DIR)),
        })
    }

    /// The media folder when known, absolute and UTF-8, so that `json!` cannot panic on it.
    fn output_dir(&self) -> Option<&Path> {
        self.output_dir
            .as_deref()
            .filter(|directory| directory.is_absolute() && directory.to_str().is_some())
    }

    /// Public Hub price entries accepted by `accept`.
    async fn prices(
        &self,
        accept: impl Fn(&PriceEntry) -> bool,
    ) -> Result<Vec<(String, PriceEntry)>> {
        #[derive(Deserialize)]
        struct Prices {
            providers: BTreeMap<String, Group>,
        }
        #[derive(Deserialize)]
        struct Group {
            models: BTreeMap<String, PriceEntry>,
        }
        let value = read_json(self.client.get(self.origin.join("/hub/api/prices")?)).await?;
        let prices: Prices = serde_json::from_value(value).context("Invalid Hub price catalog")?;
        Ok(prices
            .providers
            .into_values()
            .flat_map(|group| group.models)
            .filter(|(_, entry)| accept(entry))
            .collect())
    }

    async fn models(&self) -> Result<BTreeMap<String, Model>> {
        let mut models = BTreeMap::new();
        for (id, entry) in self.prices(|entry| entry.category == "videos").await? {
            let Some(provider) = entry.endpoints.iter().find_map(|path| match path.as_str() {
                OPENROUTER => Some(Provider::OpenRouter),
                FAL => Some(Provider::Fal),
                _ => None,
            }) else {
                continue;
            };
            let (min, max) = entry.range("video_seconds", 1.0 / 1_000_000.0);
            models.insert(
                id.clone(),
                Model {
                    id,
                    provider,
                    name: entry.display_name,
                    min_rub_per_second: min,
                    max_rub_per_second: max,
                },
            );
        }
        Ok(models)
    }

    async fn create(
        &self,
        input: Create,
        key: Option<&MediaKey>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Value> {
        let (model_id, mut body) = input.into_body()?;
        let model = self
            .models()
            .await?
            .remove(&model_id)
            .context("Unknown video model. Call video_models for current IDs")?;
        let url = match model.provider {
            Provider::OpenRouter => {
                body.insert("model".into(), model_id.into());
                self.origin.join(OPENROUTER)?
            }
            Provider::Fal => self
                .origin
                .join(&format!("{FAL}/{}", fal_model(&model.id)?))?,
        };
        let key = key.context(MISSING_KEY)?;
        ensure!(!cancelled(), "{REQUEST_CANCELLED}");
        let req = self
            .client
            .post(url)
            .bearer_auth(&key.token)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
        let uncertain = "Video submission failed. If the outcome is uncertain, do not submit again automatically";
        let response =
            send_paid(req, "CoreInfra Hub rejected the video request", uncertain).await?;
        let response = read_json_response(response, MAX_RESPONSE_BYTES)
            .await
            .context(uncertain)?;
        let id_field = if model.provider == Provider::Fal {
            "request_id"
        } else {
            "id"
        };
        let id = response[id_field].as_str().filter(|id| safe_segment(id))
            .context("Hub accepted the submission but returned no usable job ID; do not submit again automatically")?;
        let job = Job {
            provider: model.provider,
            model: model.id,
            id: id.to_owned(),
            key_id: fingerprint(&key.token),
        };
        Ok(json!({"job":job, "status":response["status"].as_str().unwrap_or("pending")}))
    }

    async fn result(&self, input: GetResult, key: Option<&MediaKey>) -> Result<Value> {
        let job = &input.job;
        ensure!(safe_segment(&job.id), "Invalid job ID");
        let path = match job.provider {
            Provider::OpenRouter => format!("{OPENROUTER}/{}", job.id),
            Provider::Fal => {
                ensure!(input.index == 0, "fal has only output index 0");
                let model = fal_model(&job.model)?;
                let (owner, rest) = model.split_once('/').context("Invalid fal model")?;
                let app = rest.split('/').next().context("Invalid fal model")?;
                format!("{FAL}/{owner}/{app}/requests/{}", job.id)
            }
        };
        let key = key.context(MISSING_KEY)?;
        ensure!(
            job.key_id == fingerprint(&key.token),
            "This video job requires its original Hub and API key"
        );
        let token = key.token.as_str();
        let url = self.origin.join(&path)?;
        let status_url = if job.provider == Provider::Fal {
            self.origin.join(&format!("{path}/status"))?
        } else {
            url.clone()
        };
        let status = read_json(self.client.get(status_url).bearer_auth(token)).await?;
        let state = status["status"]
            .as_str()
            .context("Hub response missing job status")?;
        let mut result = json!({"job":job, "status":state});
        if let Some(cost) = status.pointer("/usage/cost_rub").and_then(Value::as_number) {
            result["cost_rub"] = json!(cost);
        }
        if !state.eq_ignore_ascii_case("completed") {
            return Ok(result);
        }

        let download = input.download.unwrap_or(true);
        let request = match job.provider {
            Provider::OpenRouter => {
                if !download {
                    return Ok(result);
                }
                let mut content = self.origin.join(&format!("{path}/content"))?;
                content
                    .query_pairs_mut()
                    .append_pair("index", &input.index.to_string());
                self.client.get(content).bearer_auth(token)
            }
            Provider::Fal => {
                let completed = read_json(self.client.get(url).bearer_auth(token)).await?;
                let signed = completed
                    .pointer("/video/url")
                    .and_then(Value::as_str)
                    .context("Hub response missing video URL")?;
                let content = Url::parse(signed).context("Invalid video URL")?;
                ensure!(
                    content.origin() == self.origin.origin()
                        && content.path() == format!("{path}/content")
                        && content.username().is_empty()
                        && content.password().is_none()
                        && content.fragment().is_none()
                        && content
                            .query_pairs()
                            .any(|(name, value)| name == "t" && !value.is_empty()),
                    "Invalid Hub video download URL"
                );
                if !download {
                    result["url"] = json!(content.as_str());
                    return Ok(result);
                }
                self.client.get(content)
            }
        };
        let directory = self
            .output_dir()
            .context("Downloads directory unavailable")?;
        let response = send(request).await?;
        result["path"] = json!(save_video(directory, response).await?);
        Ok(result)
    }
}

fn safe_segment(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 256
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

fn fal_model(model: &str) -> Result<&str> {
    ensure!(
        model.contains('/') && model.split('/').all(safe_segment),
        "Invalid fal video model"
    );
    Ok(model)
}

const REQUEST_CANCELLED: &str = "Media request was cancelled before submission";
const UNREACHABLE: &str = "Could not reach CoreInfra Hub";
const HUB_TOO_SLOW: &str =
    "CoreInfra Hub did not answer in time. The request may still be processed and billed";
const LOST_RESPONSE: &str = "Connection to CoreInfra Hub was lost before a response arrived. The request may still be processed and billed";

async fn send(request: RequestBuilder) -> Result<Response> {
    let response = request.send().await.map_err(|_| anyhow!("{UNREACHABLE}"))?;
    hub_status(&response)?;
    Ok(response)
}

/// Sends a paid request, separating 4xx client rejections from timeouts and server/transport errors
/// whose billing outcome is uncertain.
async fn send_paid(request: RequestBuilder, rejected: &str, uncertain: &str) -> Result<Response> {
    let response = request.send().await.map_err(|error| {
        // Only a connect failure guarantees the request never reached the Hub.
        let cause = if error.is_connect() {
            UNREACHABLE
        } else if error.is_timeout() {
            HUB_TOO_SLOW
        } else {
            LOST_RESPONSE
        };
        anyhow::Error::msg(cause).context(uncertain.to_owned())
    })?;
    if let Err(error) = hub_status(&response) {
        let text = if response.status().is_client_error() {
            rejected
        } else {
            uncertain
        };
        return Err(error.context(text.to_owned()));
    }
    Ok(response)
}

/// The error of a non-success answer. Never reads the body.
fn hub_status(response: &Response) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if let Some(seconds) = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        bail!(
            "CoreInfra Hub returned HTTP {} (Retry-After: {seconds}s)",
            status.as_u16()
        );
    }
    // Error bodies are not shown: they may echo request credentials.
    bail!("CoreInfra Hub returned HTTP {}", status.as_u16())
}

async fn read_body(mut response: Response, limit: usize, what: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Incomplete Hub {what} response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Hub {what} response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_json(request: RequestBuilder) -> Result<Value> {
    read_json_limit(request, MAX_RESPONSE_BYTES).await
}

async fn read_json_limit(request: RequestBuilder, limit: usize) -> Result<Value> {
    read_json_response(send(request).await?, limit).await
}

async fn read_json_response(response: Response, limit: usize) -> Result<Value> {
    let bytes = read_body(response, limit, "JSON").await?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid JSON from CoreInfra Hub"))
}

/// The lowercase MIME type without parameters.
fn content_type(response: &Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map(|mime| {
            mime.to_str()
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
}

async fn save_video(dir: &Path, response: Response) -> Result<PathBuf> {
    if let Some(mime) = content_type(&response) {
        ensure!(
            ["video/mp4", "application/mp4", "application/octet-stream"].contains(&mime.as_str()),
            "Hub returned a non-MP4 response"
        );
    }
    Ok(save_download(dir, response, "video-", "mp4").await?.0)
}

/// Stream a download into `dir` under a fresh random name. Never overwrites a file.
async fn save_download(
    dir: &Path,
    mut response: Response,
    prefix: &str,
    extension: &str,
) -> Result<(PathBuf, u64)> {
    tokio::fs::create_dir_all(dir).await?;
    let file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".part")
        .tempfile_in(dir)?;
    let (file, path) = file.into_parts();
    let mut file = tokio::fs::File::from_std(file);
    let mut size = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Incomplete download from CoreInfra Hub"))?
    {
        size += chunk.len() as u64;
        file.write_all(&chunk).await?;
    }
    ensure!(size > 0, "Empty file from CoreInfra Hub");
    file.flush().await?;
    drop(file);
    let output = path.with_extension(extension);
    path.persist_noclobber(&output)?;
    Ok((output, size))
}

#[cfg(test)]
mod tests;
