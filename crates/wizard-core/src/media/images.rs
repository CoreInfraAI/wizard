//! `OpenRouter` image generation and editing through `CoreInfra` Hub. Wizard reads reference
//! pictures only from its own media folder.
use super::{
    MISSING_KEY, MediaHub, MediaKey, PriceEntry, REQUEST_CANCELLED, extra_fields, read_body,
    send_paid,
};
use alloc::collections::BTreeMap;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

pub(super) const IMAGES: &str = "/openrouter/api/v1/images";
/// Decoded reference bytes in one request. 24 MiB become 32 MiB of base64, Wizard's request cap.
const MAX_REFERENCE_BYTES: usize = 24 * 1024 * 1024;
/// Image responses carry base64 inside JSON, up to four images.
const MAX_IMAGE_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

const UNCERTAIN: &str =
    "Image generation failed. If the outcome is uncertain, do not submit again automatically";
const TOO_LARGE_REFERENCES: &str = "Reference images are too large: at most about 24 MiB together. Use smaller pictures or fewer references";
const NOT_AN_IMAGE: &str = "Reference image must be a PNG, JPEG, WebP or GIF file";

fn default_one() -> u8 {
    1
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageCreate {
    /// Image model ID from `image_models`, of the form `<author>/<model>`.
    model: String,
    /// What to generate, or how to change the reference images.
    prompt: String,
    /// Reference pictures in the media folder (Downloads/CoreInfra Media): absolute paths or
    /// relative to that folder. PNG, JPEG, WebP or GIF.
    #[serde(default)]
    reference_paths: Vec<String>,
    /// Small reference pictures as inline `data:image/...;base64,...` URIs.
    #[serde(default)]
    references: Vec<String>,
    /// Number of images, 1 to 4. Each is billed, and all must fit one response of 64 MiB.
    /// Defaults to 1. Use separate calls for different pictures.
    #[serde(default = "default_one")]
    #[schemars(range(min = 1, max = 4))]
    n: u8,
    /// Extra top-level fields of the `OpenRouter` images JSON body, such as
    /// `aspect_ratio`, `size`, `quality` or `output_format`.
    fields: Option<Map<String, Value>>,
    /// Alternative for clients that require fixed object schemas. A JSON object string.
    fields_json: Option<String>,
}

#[derive(Serialize)]
struct ImageModel {
    id: String,
    name: Option<String>,
    min_rub_per_image: Option<f64>,
    max_rub_per_image: Option<f64>,
}

#[derive(Serialize)]
struct SavedImage {
    path: PathBuf,
    content_type: &'static str,
    size_bytes: usize,
}

impl MediaHub {
    /// Image models served on the images endpoint, by ID.
    async fn image_prices(&self) -> Result<BTreeMap<String, PriceEntry>> {
        Ok(self
            .prices(|entry| {
                entry.category == "images" && entry.endpoints.iter().any(|path| path == IMAGES)
            })
            .await?
            .into_iter()
            .collect())
    }

    pub(super) async fn image_models(&self) -> Result<Value> {
        let models: Vec<_> = self
            .image_prices()
            .await?
            .into_iter()
            .map(|(id, entry)| {
                let (min, max) = entry.range("images", 1.0 / 1_000_000.0);
                ImageModel {
                    id,
                    name: entry.display_name,
                    min_rub_per_image: min,
                    max_rub_per_image: max,
                }
            })
            .collect();
        Ok(json!({"models": models, "output_dir": self.output_dir()}))
    }

    pub(super) async fn image_create(
        &self,
        input: ImageCreate,
        key: Option<&MediaKey>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Value> {
        ensure!(
            !input.prompt.trim().is_empty(),
            "Provide the image description in prompt"
        );
        ensure!((1..=4).contains(&input.n), "n must be from 1 to 4");
        let mut body = extra_fields(input.fields, input.fields_json, &["model", "prompt", "n"])?;
        if let Some(key) = ["stream", "stream_format", "response_format"]
            .into_iter()
            .find(|key| body.contains_key(*key))
        {
            bail!("{key} is not supported: image_create saves complete image files");
        }
        ensure!(
            !["input_references", "reference_paths", "references"]
                .iter()
                .any(|key| body.contains_key(*key)),
            "Pass reference pictures in reference_paths or references, not in fields"
        );
        let directory = self
            .output_dir()
            .context("Downloads directory unavailable")?
            .to_owned();
        let key = key.context(MISSING_KEY)?;
        ensure!(
            self.image_prices().await?.contains_key(&input.model),
            "Unknown image model. Call image_models for current IDs"
        );

        // The proxy daemon runs on one thread: file, base64 and JSON work runs off it.
        let paths = input.reference_paths;
        let inline = input.references;
        let dir = directory.clone();
        let payload = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let references = reference_images(&paths, inline, &dir)?;
            // Before billing, prove that the image can be saved: macOS may deny Downloads.
            std::fs::create_dir_all(&dir)
                .and_then(|()| tempfile::tempfile_in(&dir))
                .map_err(|error| {
                    anyhow!(
                        "Wizard cannot write to {} ({error}): check that Wizard may access your Downloads folder",
                        dir.display()
                    )
                })?;
            body.insert("model".into(), input.model.into());
            body.insert("prompt".into(), input.prompt.into());
            if input.n > 1 {
                body.insert("n".into(), input.n.into());
            }
            if !references.is_empty() {
                let references = references
                    .into_iter()
                    .map(|url| json!({"type":"image_url","image_url":{"url":url}}))
                    .collect();
                body.insert("input_references".into(), Value::Array(references));
            }
            Ok(serde_json::to_vec(&body)?)
        })
        .await
        .context("Image request task failed")??;

        ensure!(!cancelled(), "{REQUEST_CANCELLED}");
        let request = self
            .client
            .post(self.origin.join(IMAGES)?)
            .bearer_auth(&key.token)
            .header("content-type", "application/json")
            .body(payload);
        let response = send_paid(
            request,
            "CoreInfra Hub rejected the image request",
            UNCERTAIN,
        )
        .await?;
        let bytes = read_body(response, MAX_IMAGE_RESPONSE_BYTES, "image")
            .await
            .map_err(|error| unsaved(&[], &error))?;
        tokio::task::spawn_blocking(move || save_images(&bytes, &directory))
            .await
            .map_err(|error| unsaved(&[], &error.into()))?
    }
}

/// The reference pictures as data URLs built from the bytes Wizard read: the files first, in the
/// order given, then the inline pictures.
fn reference_images(
    paths: &[String],
    inline: Vec<String>,
    directory: &Path,
) -> Result<Vec<String>> {
    let mut urls = Vec::new();
    let mut total = 0;
    let folder = std::fs::canonicalize(directory);
    for item in paths {
        let inside = || {
            anyhow!(
                "Reference images must be inside {}: {item}",
                directory.display()
            )
        };
        let path = Path::new(item);
        let relative = [Ok(directory), folder.as_deref()]
            .into_iter()
            .find_map(|prefix| path.strip_prefix(prefix.ok()?).ok())
            .unwrap_or(path);
        // Only plain segments, checked before touching the disk: no root, drive, share or `..`.
        ensure!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
            "{}",
            inside()
        );
        let folder = folder.as_deref().map_err(|error| unreadable(error, item))?;
        let resolved = std::fs::canonicalize(folder.join(relative))
            .map_err(|error| unreadable(&error, item))?;
        ensure!(resolved.starts_with(folder), "{}", inside());
        // Opening a FIFO would wait for a writer without end.
        ensure!(
            resolved.is_file(),
            "Reference image must be a regular file: {item}"
        );
        let mut bytes = Vec::new();
        std::fs::File::open(&resolved)
            .and_then(|file| {
                file.take((MAX_REFERENCE_BYTES - total) as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| unreadable(&error, item))?;
        total += bytes.len();
        ensure!(total <= MAX_REFERENCE_BYTES, "{TOO_LARGE_REFERENCES}");
        urls.push(data_url(&bytes).with_context(|| format!("{NOT_AN_IMAGE}: {item}"))?);
    }
    for (index, uri) in inline.into_iter().enumerate() {
        let name = format!("inline reference {}", index + 1);
        let bytes = uri
            .strip_prefix("data:image/")
            .and_then(|rest| rest.split_once(";base64,"))
            .and_then(|(_, payload)| decode_base64(payload))
            .with_context(|| {
                format!("Inline reference image must be a data:image/...;base64 URI: {name}")
            })?;
        total += bytes.len();
        ensure!(total <= MAX_REFERENCE_BYTES, "{TOO_LARGE_REFERENCES}");
        urls.push(data_url(&bytes).with_context(|| format!("{NOT_AN_IMAGE}: {name}"))?);
    }
    Ok(urls)
}

/// A missing reference has its own text. Any other failure is an access problem.
fn unreadable(error: &std::io::Error, item: &str) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow!("Reference image not found: {item}")
    } else {
        anyhow!("Wizard cannot read {item}: check that Wizard may access your Downloads folder")
    }
}

/// The picture as a data URL whose type comes from its bytes. A reference must be raster.
fn data_url(bytes: &[u8]) -> Option<String> {
    let (_, mime) = image_kind(bytes).filter(|&(extension, _)| extension != "svg")?;
    Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

/// Saves each image of a 2xx answer in turn. The images saved before a failure stay, and the
/// error names them.
fn save_images(bytes: &[u8], directory: &Path) -> Result<Value> {
    let answer: Value = serde_json::from_slice(bytes)
        .map_err(|_| unsaved(&[], &anyhow!("Invalid JSON from CoreInfra Hub")))?;
    let data = answer["data"]
        .as_array()
        .filter(|data| !data.is_empty())
        .ok_or_else(|| unsaved(&[], &anyhow!("CoreInfra Hub returned no images")))?;
    let mut images = Vec::new();
    for (index, image) in data.iter().enumerate() {
        let saved = save_image(image)
            .and_then(|(bytes, extension, content_type)| {
                Ok(SavedImage {
                    path: save_bytes(directory, &bytes, extension)?,
                    content_type,
                    size_bytes: bytes.len(),
                })
            })
            .map_err(|cause| unsaved(&images, &cause.context(format!("image {}", index + 1))))?;
        images.push(saved);
    }
    Ok(json!({"images": images, "usage": answer["usage"]}))
}

/// The bytes, file extension and MIME type of one image of a Hub answer.
fn save_image(image: &Value) -> Result<(Vec<u8>, &'static str, &'static str)> {
    let bytes = image["b64_json"]
        .as_str()
        .and_then(decode_base64)
        .context("does not decode")?;
    let (extension, mime) =
        image_kind(&bytes).context("is not a PNG, JPEG, GIF, WebP or SVG file")?;
    Ok((bytes, extension, mime))
}

/// Writes bytes into `dir` under a fresh random name. Never overwrites a file.
fn save_bytes(dir: &Path, bytes: &[u8], extension: &str) -> Result<PathBuf> {
    // The folder may have been deleted while the Hub was generating.
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::Builder::new()
        .prefix("image-")
        .suffix(".part")
        .tempfile_in(dir)?;
    file.write_all(bytes)?;
    let output = file.path().with_extension(extension);
    file.persist_noclobber(&output)?;
    Ok(output)
}

/// The error for a 2xx answer whose images were not all saved: the request was billed.
fn unsaved(saved: &[SavedImage], cause: &anyhow::Error) -> anyhow::Error {
    let saved = if saved.is_empty() {
        "No image was saved".to_owned()
    } else {
        let paths: Vec<_> = saved
            .iter()
            .map(|image| image.path.display().to_string())
            .collect();
        format!("Only {} saved", paths.join(", "))
    };
    anyhow!(
        "{saved} ({cause:#}). The request may have been billed. Do not submit again automatically"
    )
}

/// Standard base64. Line breaks are tolerated: answers and clients wrap long values.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(text.split_ascii_whitespace().collect::<String>())
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

/// The file extension and MIME type of an image, from its leading bytes. SVG, for Hub answers
/// only, by its first element, so an HTML page that contains an inline SVG is not an image.
fn image_kind(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some(("png", "image/png")),
        [0xff, 0xd8, 0xff, ..] => Some(("jpg", "image/jpeg")),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some(("gif", "image/gif")),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(("webp", "image/webp")),
        _ => svg_root(bytes).then_some(("svg", "image/svg+xml")),
    }
}

/// Whether the first element of the XML document, after any declaration, comment, doctype and
/// whitespace, is `svg`.
fn svg_root(bytes: &[u8]) -> bool {
    use quick_xml::{events::Event, reader::Reader};

    let Ok(text) = core::str::from_utf8(bytes) else {
        return false;
    };
    let mut reader = Reader::from_str(text.trim_start_matches('\u{feff}'));
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                return element.local_name().as_ref() == "svg";
            }
            Ok(Event::Text(text)) if text.trim_ascii().is_empty() => {}
            Ok(Event::Decl(_) | Event::Comment(_) | Event::DocType(_) | Event::PI(_)) => {}
            Ok(_) | Err(_) => return false,
        }
    }
}
