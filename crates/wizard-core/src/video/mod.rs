//! Stateless video tools. Job references are carried by the caller and contain no credential.
pub mod install;
pub mod server;

use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::{future::Future, time::Duration};
use reqwest::{Client, RequestBuilder, Response, Url};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt as _;
use wizard_settings::VideoConnection;

pub const ADDRESS: &str = "127.0.0.1:33789";
pub const URL: &str = "http://127.0.0.1:33789/mcp";
const OPENROUTER: &str = "/openrouter/api/v1/videos";
const FAL: &str = "/fal/api";

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
        let mut body = match (self.fields, self.fields_json) {
            (Some(_), Some(_)) => bail!("Use either fields or fields_json"),
            (Some(fields), None) => fields,
            (None, Some(text)) => serde_json::from_str(&text)
                .map_err(|_| anyhow!("fields_json must encode a JSON object"))?,
            (None, None) => Map::new(),
        };
        ensure!(
            !body.contains_key("model") && !body.contains_key("prompt"),
            "Put model and prompt in the named tool arguments"
        );
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
struct Job {
    provider: Provider,
    model: String,
    id: String,
    hub_id: String,
    key_id: String,
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

#[derive(Clone)]
pub struct VideoHub {
    client: Client,
    origin: Url,
    output_dir: Option<PathBuf>,
}

impl VideoHub {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(wizard_proxy::user_agent())
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(180))
                .build()
                .map_err(|_| anyhow!("Could not initialize the video HTTP client"))?,
            origin: Url::parse(crate::hub::HUB_ORIGIN)?,
            output_dir: dirs::download_dir()
                .or_else(|| dirs::home_dir().map(|dir| dir.join("Downloads")))
                .map(|dir| dir.join("CoreInfra Video")),
        })
    }

    async fn models(&self) -> Result<BTreeMap<String, Model>> {
        #[derive(Deserialize)]
        struct Prices {
            providers: BTreeMap<String, Group>,
        }
        #[derive(Deserialize)]
        struct Group {
            models: BTreeMap<String, Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            category: String,
            endpoints: Vec<String>,
            display_name: Option<String>,
            prices: Rates,
            lowest_prices: Option<Rates>,
        }
        #[derive(Deserialize)]
        struct Rates {
            video_seconds: Option<f64>,
        }
        let value = read_json(self.client.get(self.origin.join("/hub/api/prices")?)).await?;
        let prices: Prices = serde_json::from_value(value).context("Invalid Hub price catalog")?;
        let mut models = BTreeMap::new();
        for (id, entry) in prices
            .providers
            .into_values()
            .flat_map(|group| group.models)
        {
            if entry.category != "videos" {
                continue;
            }
            let Some(provider) = entry.endpoints.iter().find_map(|path| match path.as_str() {
                OPENROUTER => Some(Provider::OpenRouter),
                FAL => Some(Provider::Fal),
                _ => None,
            }) else {
                continue;
            };
            let max = entry.prices.video_seconds.map(|p| p / 1_000_000.0);
            let min = entry
                .lowest_prices
                .and_then(|p| p.video_seconds)
                .map(|p| p / 1_000_000.0)
                .or(max);
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
        connection: &VideoConnection,
        resolve_token: impl Future<Output = Result<String>>,
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
        let token = resolve_token.await?;
        let req = self
            .client
            .post(url)
            .bearer_auth(&token)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
        let response = read_json(req).await.context("Video submission failed. If the outcome is uncertain, do not submit again automatically")?;
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
            hub_id: connection.hub_id.clone(),
            key_id: connection.key_id.clone(),
        };
        Ok(json!({"job":job, "status":response["status"].as_str().unwrap_or("pending")}))
    }

    async fn result(
        &self,
        input: GetResult,
        connection: &VideoConnection,
        resolve_token: impl Future<Output = Result<String>>,
    ) -> Result<Value> {
        let job = &input.job;
        ensure!(
            job.key_id == connection.key_id && job.hub_id == connection.hub_id,
            "Reconnect the original Hub and video key in Wizard to read this job"
        );
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
        let token = resolve_token.await?;
        let url = self.origin.join(&path)?;
        let status_url = if job.provider == Provider::Fal {
            self.origin.join(&format!("{path}/status"))?
        } else {
            url.clone()
        };
        let status = read_json(self.client.get(status_url).bearer_auth(&token)).await?;
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
                self.client.get(content).bearer_auth(&token)
            }
            Provider::Fal => {
                let completed = read_json(self.client.get(url).bearer_auth(&token)).await?;
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
            .output_dir
            .as_deref()
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

async fn send(request: RequestBuilder) -> Result<Response> {
    let response = request
        .send()
        .await
        .map_err(|_| anyhow!("Could not reach CoreInfra Hub"))?;
    let status = response.status();
    if !status.is_success() {
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
        bail!("CoreInfra Hub returned HTTP {}", status.as_u16());
    }
    Ok(response)
}

async fn read_json(request: RequestBuilder) -> Result<Value> {
    let mut response = send(request).await?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Incomplete Hub response"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 8 * 1024 * 1024,
            "Hub JSON response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid JSON from CoreInfra Hub"))
}

async fn save_video(dir: &Path, mut response: Response) -> Result<PathBuf> {
    if let Some(mime) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let mime = mime
            .to_str()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        ensure!(
            ["video/mp4", "application/mp4", "application/octet-stream"]
                .iter()
                .any(|t| mime.eq_ignore_ascii_case(t)),
            "Hub returned a non-MP4 response"
        );
    }
    tokio::fs::create_dir_all(dir).await?;
    let file = tempfile::Builder::new()
        .prefix("video-")
        .suffix(".part")
        .tempfile_in(dir)?;
    let (file, path) = file.into_parts();
    let mut file = tokio::fs::File::from_std(file);
    let mut size = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Incomplete video download"))?
    {
        size += chunk.len() as u64;
        file.write_all(&chunk).await?;
    }
    ensure!(size > 0, "Empty video from CoreInfra Hub");
    file.flush().await?;
    drop(file);
    let output = path.with_extension("mp4");
    path.persist_noclobber(&output)?;
    Ok(output)
}

#[cfg(test)]
mod tests;
