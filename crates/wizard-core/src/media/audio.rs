//! `OpenRouter` speech synthesis and transcription through `CoreInfra` Hub.
//! Audio input is inline only: Wizard never reads a caller-supplied path.
use super::{
    MISSING_KEY, MediaHub, MediaKey, PriceEntry, content_type, extra_fields, read_json,
    safe_segment, save_download, send,
};
use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub(super) const SPEECH: &str = "/openrouter/api/v1/audio/speech";
pub(super) const TRANSCRIPTIONS: &str = "/openrouter/api/v1/audio/transcriptions";
/// Hub prices are RUB per million characters or seconds.
const PER_1K_CHARACTERS: f64 = 1.0 / 1_000.0;
const PER_MINUTE: f64 = 60.0 / 1_000_000.0;
/// The saved file is binary audio; event streams cannot be represented.
const STREAMING: &[&str] = &["stream", "stream_format"];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SpeechModels {
    /// `<author>/<slug>` from the list, to get its voices, formats and other parameters.
    /// Omit to list all speech models.
    model: Option<String>,
}

#[derive(Serialize)]
struct SpeechModel {
    id: String,
    name: Option<String>,
    response_formats: Vec<Value>,
    voice_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<Value>,
    parameters: Vec<String>,
    min_rub_per_1k_characters: Option<f64>,
    max_rub_per_1k_characters: Option<f64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpeechTurn {
    text: String,
    voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SpeechCreate {
    /// Speech model ID from `speech_models`, such as `elevenlabs/eleven-v3`.
    model: String,
    /// Text to synthesize. Omit when passing multi-speaker `turns`.
    input: Option<String>,
    /// Multi-speaker dialogue turns (`[{text, voice, instructions?}]`), when the model supports turns.
    turns: Option<Vec<SpeechTurn>>,
    /// One of the model's voices from `speech_models` with `model`.
    voice: Option<String>,
    /// Audio format supported by the model, such as `mp3` or `pcm`.
    response_format: Option<String>,
    /// Playback speed within the model's supported range.
    speed: Option<f64>,
    /// Extra top-level fields of the `OpenRouter` speech JSON body, such as `instructions`.
    fields: Option<Map<String, Value>>,
    /// Alternative for clients that require fixed object schemas. A JSON object string.
    fields_json: Option<String>,
}

#[derive(Serialize)]
struct TranscriptionModel {
    id: String,
    name: Option<String>,
    min_rub_per_minute: Option<f64>,
    max_rub_per_minute: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TranscriptionCreate {
    /// Transcription model ID from `transcription_models`, such as `openai/whisper-large-v3-turbo`.
    model: String,
    /// The audio file encoded as standard base64, without a `data:` prefix.
    audio_base64: String,
    /// Audio container format, such as `wav`, `mp3`, `flac`, `m4a`, `ogg`, `webm` or `aac`.
    format: String,
    /// ISO-639-1 language of the audio, such as `ru`. Detected when omitted.
    language: Option<String>,
    /// Context or spelling hints for the transcript.
    prompt: Option<String>,
    /// Output format supported by the model, such as `json` or `verbose_json`.
    response_format: Option<String>,
    /// Extra top-level fields of the `OpenRouter` transcription JSON body, such as `temperature`.
    fields: Option<Map<String, Value>>,
    /// Alternative for clients that require fixed object schemas. A JSON object string.
    fields_json: Option<String>,
}

/// `<author>/<slug>` made of path-safe segments.
fn model_id(model: &str) -> Result<(&str, &str)> {
    model
        .split_once('/')
        .filter(|(author, slug)| safe_segment(author) && safe_segment(slug))
        .context("Expected a model ID of the form <author>/<slug>")
}

/// A short format name, such as `mp3` or `verbose_json`.
fn format_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The speech output parameters of an `OpenRouter` model description.
fn speech_parameters(model: &Value) -> Map<String, Value> {
    let speech = |modalities: &Value| {
        modalities.as_array().and_then(|modalities| {
            modalities
                .iter()
                .find(|m| m["type"] == "speech")
                .and_then(|m| m["supported_parameters"].as_object())
                .cloned()
        })
    };
    speech(&model["output_modalities"])
        .or_else(|| {
            model["endpoints"]
                .as_array()?
                .iter()
                .find_map(|endpoint| speech(&endpoint["output_modalities"]))
        })
        .unwrap_or_default()
}

/// The audio file extension for a response. Never trusts an arbitrary caller string.
fn audio_extension(mime: &str, requested: Option<&str>) -> Option<&'static str> {
    let known = match mime {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" => "mp3",
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => "wav",
        "audio/pcm" | "audio/l16" | "audio/x-pcm" | "audio/basic" => "pcm",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/aac" | "audio/x-aac" => "aac",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/webm" => "webm",
        _ => {
            let fallback = if mime.starts_with("audio/") {
                "audio"
            } else if mime == "application/octet-stream" {
                "bin"
            } else {
                return None;
            };
            let requested =
                requested.and_then(|format| match format.to_ascii_lowercase().as_str() {
                    "mp3" => Some("mp3"),
                    "wav" => Some("wav"),
                    "pcm" => Some("pcm"),
                    "ogg" => Some("ogg"),
                    "opus" => Some("opus"),
                    "flac" => Some("flac"),
                    "aac" => Some("aac"),
                    "m4a" => Some("m4a"),
                    "webm" => Some("webm"),
                    _ => None,
                });
            return Some(requested.unwrap_or(fallback));
        }
    };
    Some(known)
}

impl MediaHub {
    async fn audio_prices(&self, endpoint: &str) -> Result<BTreeMap<String, PriceEntry>> {
        Ok(self
            .prices(|entry| entry.endpoints.iter().any(|path| path == endpoint))
            .await?
            .into_iter()
            .collect())
    }

    pub(super) async fn speech_models(&self, input: SpeechModels) -> Result<Value> {
        #[derive(Deserialize)]
        struct Catalog {
            data: Vec<Value>,
        }
        if let Some(model) = input.model {
            let (author, slug) = model_id(&model)?;
            let url = self
                .origin
                .join(&format!("{SPEECH}/models/{author}/{slug}"))?;
            let (detail, prices) =
                tokio::try_join!(read_json(self.client.get(url)), self.audio_prices(SPEECH))?;
            let id = detail["id"].as_str().unwrap_or(&model).to_owned();
            let (min, max) = prices.get(&id).map_or((None, None), |p| {
                p.range("input_characters", PER_1K_CHARACTERS)
            });
            return Ok(json!({"model": {
                "id": id,
                "name": detail["name"].as_str().or_else(|| prices.get(&id).and_then(|p| p.display_name.as_deref())),
                "description": detail["description"],
                "input_modalities": detail["input_modalities"],
                "supported_parameters": speech_parameters(&detail),
                "min_rub_per_1k_characters": min,
                "max_rub_per_1k_characters": max,
            }}));
        }
        let url = self.origin.join(&format!("{SPEECH}/models"))?;
        let (catalog, prices) =
            tokio::try_join!(read_json(self.client.get(url)), self.audio_prices(SPEECH))?;
        let catalog: Catalog =
            serde_json::from_value(catalog).context("Invalid Hub speech model catalog")?;
        let mut models = Vec::new();
        for model in catalog.data {
            let Some(id) = model["id"].as_str() else {
                continue;
            };
            let parameters = speech_parameters(&model);
            let price = prices.get(id);
            let (min, max) = price.map_or((None, None), |p| {
                p.range("input_characters", PER_1K_CHARACTERS)
            });
            models.push(SpeechModel {
                id: id.to_owned(),
                name: model["name"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| price.and_then(|p| p.display_name.clone())),
                response_formats: parameters
                    .get("response_format")
                    .and_then(|f| f["values"].as_array())
                    .cloned()
                    .unwrap_or_default(),
                voice_count: parameters
                    .get("voice")
                    .and_then(|v| v["values"].as_array())
                    .map(Vec::len),
                speed: parameters
                    .get("speed")
                    .filter(|s| s["type"] == "range")
                    .map(|s| json!({"min": s["min"], "max": s["max"]})),
                parameters: parameters.keys().cloned().collect(),
                min_rub_per_1k_characters: min,
                max_rub_per_1k_characters: max,
            });
        }
        Ok(json!({"models": models}))
    }

    pub(super) async fn speech_create(
        &self,
        input: SpeechCreate,
        key: Option<&MediaKey>,
    ) -> Result<Value> {
        model_id(&input.model)?;
        let speech_input = match (input.input, input.turns) {
            (Some(text), None) => {
                ensure!(
                    !text.trim().is_empty(),
                    "Provide the text to speak in input"
                );
                Value::String(text)
            }
            (None, Some(turns)) => {
                ensure!(
                    !turns.is_empty()
                        && turns
                            .iter()
                            .all(|t| !t.text.trim().is_empty() && !t.voice.trim().is_empty()),
                    "Each turn must include non-empty text and voice"
                );
                serde_json::to_value(turns)?
            }
            _ => bail!("Provide either input or turns"),
        };
        let named = ["model", "input", "voice", "response_format", "speed"];
        let mut body = extra_fields(input.fields, input.fields_json, &named)?;
        ensure!(
            !STREAMING.iter().any(|key| body.contains_key(*key)),
            "Streaming is not supported: speech_create saves a complete audio file"
        );
        if let Some(format) = &input.response_format {
            ensure!(format_name(format), "Invalid response_format");
        }
        if let Some(speed) = input.speed {
            ensure!(
                speed.is_finite() && speed > 0.0,
                "speed must be a positive number"
            );
        }
        body.insert("model".into(), input.model.into());
        body.insert("input".into(), speech_input);
        for (name, value) in [
            ("voice", input.voice),
            ("response_format", input.response_format.clone()),
        ] {
            if let Some(value) = value {
                body.insert(name.into(), value.into());
            }
        }
        if let Some(speed) = input.speed {
            body.insert("speed".into(), speed.into());
        }
        let directory = self
            .output_dir
            .as_deref()
            .context("Downloads directory unavailable")?;
        let key = key.context(MISSING_KEY)?;
        let request = self
            .client
            .post(self.origin.join(SPEECH)?)
            .bearer_auth(&key.token)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
        let uncertain = "Speech synthesis failed. If the outcome is uncertain, do not submit again automatically";
        let response = send(request).await.context(uncertain)?;
        let mime = content_type(&response).unwrap_or_default();
        let extension = audio_extension(&mime, input.response_format.as_deref())
            .context("Hub returned a non-audio response")?;
        let (path, size) = save_download(directory, response, "speech-", extension)
            .await
            .context(uncertain)?;
        Ok(json!({"path": path, "content_type": mime, "size_bytes": size}))
    }

    pub(super) async fn transcription_models(&self) -> Result<Value> {
        let models: Vec<_> = self
            .audio_prices(TRANSCRIPTIONS)
            .await?
            .into_iter()
            .map(|(id, entry)| {
                let (min, max) = entry.range("audio_seconds", PER_MINUTE);
                TranscriptionModel {
                    id,
                    name: entry.display_name,
                    min_rub_per_minute: min,
                    max_rub_per_minute: max,
                }
            })
            .collect();
        Ok(json!({"models": models}))
    }

    pub(super) async fn transcription_create(
        &self,
        input: TranscriptionCreate,
        key: Option<&MediaKey>,
    ) -> Result<Value> {
        model_id(&input.model)?;
        let format = input.format.to_ascii_lowercase();
        ensure!(format_name(&format), "Invalid audio format");
        // Clients often produce data URIs; Hub expects the bare base64 payload.
        let audio = match input.audio_base64.split_once(";base64,") {
            Some((scheme, data)) if scheme.starts_with("data:") => data,
            _ => input.audio_base64.as_str(),
        };
        let audio: String = audio.split_ascii_whitespace().collect();
        let decoded = STANDARD
            .decode(&audio)
            .map_err(|_| anyhow!("audio_base64 must be standard base64 audio data"))?;
        ensure!(!decoded.is_empty(), "audio_base64 is empty");
        drop(decoded);
        let named = [
            "model",
            "input_audio",
            "language",
            "prompt",
            "response_format",
        ];
        let mut body = extra_fields(input.fields, input.fields_json, &named)?;
        ensure!(
            !STREAMING.iter().any(|key| body.contains_key(*key)),
            "Streaming is not supported: transcription_create returns the complete transcript"
        );
        if let Some(format) = &input.response_format {
            ensure!(format_name(format), "Invalid response_format");
        }
        body.insert("model".into(), input.model.into());
        body.insert(
            "input_audio".into(),
            json!({"data": audio, "format": format}),
        );
        for (name, value) in [
            ("language", input.language),
            ("prompt", input.prompt),
            ("response_format", input.response_format),
        ] {
            if let Some(value) = value {
                body.insert(name.into(), value.into());
            }
        }
        let key = key.context(MISSING_KEY)?;
        let request = self
            .client
            .post(self.origin.join(TRANSCRIPTIONS)?)
            .bearer_auth(&key.token)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body)?);
        let uncertain =
            "Transcription failed. If the outcome is uncertain, do not submit again automatically";
        let Value::Object(mut transcript) = read_json(request).await.context(uncertain)? else {
            bail!("Unexpected transcript from CoreInfra Hub");
        };
        if !transcript.contains_key("cost_rub")
            && let Some(cost) = transcript
                .get("usage")
                .and_then(|usage| usage.get("cost_rub"))
                .filter(|cost| cost.is_number())
                .cloned()
        {
            transcript.insert("cost_rub".into(), cost);
        }
        Ok(Value::Object(transcript))
    }
}
