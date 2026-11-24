use super::*;
use crate::config_files::toml;
use alloc::sync::Arc;
use core::convert::Infallible;
use http_body_util::{BodyExt as _, Full};
use hyper::{
    Request, Response as HttpResponse,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use reqwest::StatusCode;
use std::sync::{Mutex, Once};
use tokio::{net::TcpListener, task::JoinHandle};

const OR_MODEL: &str = "example/cinema";
const FAL_MODEL: &str = "example/flux/image-to-video";
const KEY: &str = "fixture-upstream-secret";
const LOCAL: &str = "fixture-local-grant";
const MP4: &[u8] = include_bytes!("../../tests/fixtures/blue.mp4");
const TTS_MODEL: &str = "example/voice";
const STT_MODEL: &str = "example/scribe";
const MP3: &[u8] = b"ID3\x04fixture-mp3";

fn key(token: &str) -> MediaKey {
    MediaKey {
        token: token.into(),
    }
}

struct Seen {
    method: String,
    path: String,
    auth: Option<String>,
    body: Value,
}

struct Fixture {
    hub: MediaHub,
    dir: tempfile::TempDir,
    requests: Arc<Mutex<Vec<Seen>>>,
    fake_hub: JoinHandle<()>,
}

struct RunningServer {
    url: String,
    bearer: String,
    task: JoinHandle<()>,
}

fn catalog() -> Value {
    json!({"providers":{"test":{"models":{
        OR_MODEL:{"category":"videos","display_name":"Cinema","endpoints":[OPENROUTER],"prices":{"video_seconds":2_500_000},"lowest_prices":{"video_seconds":1_000_000}},
        FAL_MODEL:{"category":"videos","endpoints":[FAL],"prices":{"video_seconds":12_000_000}},
        "ignored":{"category":"text","endpoints":["/text"],"prices":{}},
        TTS_MODEL:{"category":"speech","display_name":"Voice","endpoints":[audio::SPEECH],"prices":{"input_characters":5000},"lowest_prices":{"input_characters":2500}},
        STT_MODEL:{"category":"speech_recognition","display_name":"Scribe","endpoints":[audio::TRANSCRIPTIONS],"prices":{"audio_seconds":10_000}},
        "other/tts":{"category":"speech","endpoints":["/openai/api/v1/audio/speech"],"prices":{"input_characters":1}}
    }}}})
}

fn speech_model(id: &str) -> Value {
    let parameters = json!({
        "response_format":{"type":"enum","values":["mp3","pcm"]},
        "speed":{"type":"range","min":0.5,"max":2},
        "voice":{"type":"enum","values":["alice","bob","carol"]}
    });
    json!({"id":id,"name":"Example: Voice","description":"Fixture speech model",
        "input_modalities":[{"type":"text","turns":"optional"}],
        "output_modalities":[{"type":"speech","supported_parameters":parameters}],
        "endpoints":[{"provider_slug":"coreinfra","output_modalities":[{"type":"speech","supported_parameters":parameters}]}]})
}

/// Audio responses of the fake Hub, selected by the request.
fn audio_answer(path: &str, body: &Value) -> Option<(&'static str, Vec<u8>)> {
    if path == audio::SPEECH {
        return Some(match body["input"].as_str() {
            Some("html") => ("text/html; charset=utf-8", b"<html>error</html>".to_vec()),
            Some("json") => ("application/json", br#"{"error":"fixture"}"#.to_vec()),
            Some("octet") => ("application/octet-stream", MP3.to_vec()),
            Some("empty") => ("audio/mpeg", Vec::new()),
            _ if body["response_format"] == "pcm" => ("audio/pcm;rate=24000", vec![0, 1, 2, 3]),
            _ => ("audio/mpeg", MP3.to_vec()),
        });
    }
    if path == audio::TRANSCRIPTIONS {
        let payload = if body["response_format"] == "verbose_json" {
            json!({
                "text": "привет",
                "segments": [{"start": 0.0, "end": 1.0, "text": "привет"}],
                "usage": {"cost_rub": 0.25, "seconds": 3}
            })
        } else {
            json!({"text":"привет","usage":{"cost_rub":0.25,"seconds":3}})
        };
        return Some(("application/json", payload.to_string().into_bytes()));
    }
    None
}

impl Fixture {
    async fn new() -> Result<Self> {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = crate::hub::install_crypto_provider();
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let dir = tempfile::tempdir()?;
        let mut hub = MediaHub::new()?;
        hub.origin = Url::parse(&origin)?;
        hub.output_dir = Some(dir.path().join("downloads"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let fake_hub = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let requests = Arc::clone(&observed);
                let origin = origin.clone();
                tokio::spawn(async move {
                    let handle = service_fn(move |req: Request<Incoming>| {
                        let requests = Arc::clone(&requests);
                        let origin = origin.clone();
                        async move {
                            let (head, body) = req.into_parts();
                            let path = head.uri.to_string();
                            let auth = head
                                .headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned);
                            let bytes = body.collect().await.expect("body").to_bytes();
                            let body =
                                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
                            let mut requests = requests.lock().expect("requests");
                            let polls = requests.iter().filter(|r: &&Seen| r.path == path).count();
                            requests.push(Seen {
                                method: head.method.to_string(),
                                path: path.clone(),
                                auth: auth.clone(),
                                body: body.clone(),
                            });
                            let mut code = 200;
                            let mut redirect = false;
                            let authorized = matches!(
                                auth.as_deref(),
                                Some("Bearer fixture-upstream-secret" | "Bearer fixture-claude")
                            );
                            if authorized
                                && head.method == "POST"
                                && let Some((mime, bytes)) = audio_answer(&path, &body)
                            {
                                return Ok(HttpResponse::builder()
                                    .header("content-type", mime)
                                    .body(Full::new(Bytes::from(bytes)))
                                    .expect("audio response"));
                            }
                            let answer = if path == "/hub/api/prices" && auth.is_none() {
                                catalog()
                            } else if path == format!("{}/models", audio::SPEECH) && auth.is_none()
                            {
                                json!({"data":[speech_model(TTS_MODEL), speech_model("example/unpriced")]})
                            } else if path == format!("{}/models/{TTS_MODEL}", audio::SPEECH)
                                && auth.is_none()
                            {
                                speech_model(TTS_MODEL)
                            } else if path.ends_with("/content?t=fixture") && auth.is_none()
                                || path.ends_with("/content?index=1")
                                    && auth.as_deref() == Some("Bearer fixture-upstream-secret")
                            {
                                return Ok::<_, Infallible>(HttpResponse::new(Full::new(
                                    Bytes::from_static(MP4),
                                )));
                            } else if !matches!(
                                auth.as_deref(),
                                Some("Bearer fixture-upstream-secret" | "Bearer fixture-claude")
                            ) {
                                code = 401;
                                json!({})
                            } else if path == OPENROUTER && head.method == "POST" {
                                match body["prompt"].as_str() {
                                    Some("redirect") => {
                                        code = 307;
                                        redirect = true;
                                        json!({})
                                    }
                                    Some("reject") => {
                                        code = 422;
                                        json!({"error":KEY})
                                    }
                                    _ => json!({"id":"job-1","status":"pending"}),
                                }
                            } else if path == "/fal/api/example/flux/image-to-video"
                                && head.method == "POST"
                            {
                                json!({"request_id":"f-1","status":"IN_QUEUE","status_url":"https://untrusted.invalid/status"})
                            } else if path == format!("{OPENROUTER}/job-1") {
                                json!({"status":if polls==0 {"in_progress"} else {"completed"},"usage":{"cost_rub":6.5}})
                            } else if path.ends_with("/f-1/status")
                                || path.ends_with("/bad-link/status")
                            {
                                json!({"status":"COMPLETED"})
                            } else if path.ends_with("/f-1") {
                                json!({"video":{"url":format!("{origin}{path}/content?t=fixture")}})
                            } else if path.ends_with("/bad-link") {
                                json!({"video":{"url":"https://untrusted.invalid/content?t=fixture"}})
                            } else if path.ends_with("/failed") {
                                json!({"status":"failed"})
                            } else {
                                code = 404;
                                json!({})
                            };
                            let mut response = HttpResponse::builder()
                                .status(code)
                                .header("content-type", "application/json");
                            if redirect {
                                response =
                                    response.header("location", format!("{origin}/forbidden"));
                            }
                            Ok(response
                                .body(Full::new(Bytes::from(answer.to_string())))
                                .expect("response"))
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), handle)
                        .await;
                });
            }
        });
        Ok(Self {
            hub,
            dir,
            requests,
            fake_hub,
        })
    }

    async fn start(&self, key: Option<MediaKey>) -> Result<RunningServer> {
        let path = self.dir.path().join("profile/config.toml");
        let config = path.clone();
        let local = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/mcp", local.local_addr()?);
        let task = server::start(local, self.hub.clone(), key, move |bearer| {
            install::sync_at(&config, None, Some(bearer))
        })
        .await?;
        let doc = toml::read(&path)?;
        let bearer = toml::get_string(
            &doc,
            &[
                "mcp_servers",
                "coreinfra_media",
                "http_headers",
                "Authorization",
            ],
        )
        .and_then(|header| header.strip_prefix("Bearer "))
        .context("Installed MCP authorization")?
        .to_owned();
        Ok(RunningServer { url, bearer, task })
    }

    fn request(&self, url: &str, method: &str, params: &Value) -> RequestBuilder {
        self.hub
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .body(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string())
    }

    async fn call(&self, server: &RunningServer, tool: &str, args: Value) -> Result<Value> {
        let response = self
            .request(
                &server.url,
                "tools/call",
                &json!({"name":tool,"arguments":args}),
            )
            .bearer_auth(&server.bearer)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::OK,
            "MCP returned {}",
            response.status()
        );
        ensure!(
            !response.headers().contains_key("mcp-session-id"),
            "Unexpected MCP session"
        );
        let data: Value = serde_json::from_slice(&response.bytes().await?)?;
        ensure!(!data.to_string().contains(KEY), "Leaked Hub credential");
        ensure!(data.get("error").is_none(), "MCP error: {data}");
        Ok(data["result"].clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.fake_hub.abort();
    }
}

#[tokio::test]
async fn caller_bearer_selects_paid_key_on_shared_endpoint() -> Result<()> {
    let f = Fixture::new().await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let mut server = RunningServer {
        url: format!("http://{addr}/mcp"),
        bearer: "local-codex".into(),
        task: tokio::spawn(server::serve(
            listener,
            addr,
            f.hub.clone(),
            vec![
                ("local-codex".into(), Some(key(KEY))),
                ("local-claude".into(), Some(key("fixture-claude"))),
            ],
        )),
    };
    let args = json!({"model":OR_MODEL,"prompt":"river"});
    let first =
        f.call(&server, "video_create", args.clone()).await?["structuredContent"]["job"].clone();
    server.bearer = "local-claude".into();
    let second = f.call(&server, "video_create", args).await?["structuredContent"]["job"].clone();
    assert_eq!(first["key_id"], fingerprint(KEY));
    assert_eq!(second["key_id"], fingerprint("fixture-claude"));
    let denied = f
        .call(&server, "video_result", json!({"job":first}))
        .await?;
    assert_eq!(denied["isError"], true);
    let requests = f.requests.lock().expect("requests");
    let auth: Vec<_> = requests
        .iter()
        .filter(|r| r.path == OPENROUTER)
        .map(|r| r.auth.as_deref())
        .collect();
    assert_eq!(
        auth,
        [
            Some("Bearer fixture-upstream-secret"),
            Some("Bearer fixture-claude")
        ]
    );
    assert!(!requests.iter().any(|r| r.path.ends_with("/job-1")));
    server.task.abort();
    Ok(())
}

#[tokio::test]
async fn both_providers_fields_and_resume_across_restart() -> Result<()> {
    let fixture = Fixture::new().await?;
    let first = fixture.start(Some(key(KEY))).await?;
    let catalog = fixture.call(&first, "video_models", json!({})).await?;
    let models = catalog["structuredContent"]["models"]
        .as_array()
        .context("models")?;
    assert_eq!(models.len(), 2);
    let model = models
        .iter()
        .find(|m| m["id"] == OR_MODEL)
        .context("model")?;
    assert_eq!(model["min_rub_per_second"], 1.0);
    assert_eq!(model["max_rub_per_second"], 2.5);
    let fields = json!({"duration":5,"generate_audio":false,"seed":0,"nested":[null,{"a":[1,2.25]}],
        "provider":{"options":{"example":{"guidance_scale":4.5}}},"frame_images":[{"image_url":{"url":"data:image/png;base64,AAAA"},"frame_type":"first_frame","type":"image_url"}]});
    let first_job = fixture
        .call(
            &first,
            "video_create",
            json!({"model":OR_MODEL,"prompt":"river","fields":fields}),
        )
        .await?["structuredContent"]["job"]
        .clone();
    let fal_fields = json!({"image_url":"data:image/png;base64,AAAA","extra":{"a":null},"seed":0,"generate_audio":false});
    let fal_job = fixture
        .call(
            &first,
            "video_create",
            json!({"model":FAL_MODEL,"prompt":"waves","fields_json":fal_fields.to_string()}),
        )
        .await?["structuredContent"]["job"]
        .clone();
    assert_eq!(first_job["id"], "job-1");
    assert_eq!(fal_job["id"], "f-1");
    let pending = fixture
        .call(&first, "video_result", json!({"job":first_job}))
        .await?;
    assert_eq!(pending["structuredContent"]["status"], "in_progress");
    first.task.abort();
    let _ = first.task.await;
    let second = fixture.start(Some(key(KEY))).await?;
    assert_ne!(first.bearer, second.bearer);
    assert_eq!(
        fixture
            .request(&second.url, "tools/list", &json!({}))
            .bearer_auth(&first.bearer)
            .send()
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let completed = fixture
        .call(&second, "video_result", json!({"job":first_job,"index":1}))
        .await?["structuredContent"]
        .clone();
    assert_eq!(completed["cost_rub"], 6.5);
    let path = Path::new(completed["path"].as_str().context("OpenRouter file")?);
    assert!(path.starts_with(fixture.dir.path()));
    assert_eq!(std::fs::read(path)?, MP4);
    let fal = fixture
        .call(&second, "video_result", json!({"job":fal_job}))
        .await?["structuredContent"]
        .clone();
    assert_eq!(
        std::fs::read(fal["path"].as_str().context("fal file")?)?,
        MP4
    );
    let link = fixture
        .call(
            &second,
            "video_result",
            json!({"job":fal_job,"download":false}),
        )
        .await?;
    assert!(
        link["structuredContent"]["url"]
            .as_str()
            .context("signed URL")?
            .ends_with("/content?t=fixture")
    );
    let records = fixture.requests.lock().expect("requests");
    let submitted: Vec<_> = records.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(submitted.len(), 2);
    let mut expected = fields;
    expected["model"] = json!(OR_MODEL);
    expected["prompt"] = json!("river");
    assert_eq!(submitted[0].body, expected);
    let mut expected = fal_fields;
    expected["prompt"] = json!("waves");
    assert_eq!(submitted[1].body, expected);
    assert!(
        submitted
            .iter()
            .all(|r| r.auth.as_deref() == Some("Bearer fixture-upstream-secret"))
    );
    Ok(())
}

#[tokio::test]
async fn validates_before_billing_and_confines_credentials() -> Result<()> {
    let fixture = Fixture::new().await?;
    let server = fixture.start(Some(key(KEY))).await?;
    let unauthorized = fixture.hub.client.post(&server.url).send().await?;
    assert_eq!(unauthorized.status(), 401);
    for (header, value) in [
        ("Origin", "https://elsewhere.example"),
        ("Host", "hub.coreinfra.ai"),
    ] {
        assert!(
            fixture
                .request(&server.url, "tools/list", &json!({}))
                .bearer_auth(&server.bearer)
                .header(header, value)
                .send()
                .await?
                .status()
                .is_client_error()
        );
    }
    assert!(fixture.requests.lock().expect("requests").is_empty());
    for extra in [
        json!({"fields":{"prompt":"changed"}}),
        json!({"fields":{"model":"other"}}),
        json!({"fields_json":"[]"}),
        json!({"fields":{},"fields_json":"{}"}),
        json!({"fields_json":"{"}),
        json!({"url":"https://elsewhere.example"}),
    ] {
        let mut args = json!({"model":OR_MODEL,"prompt":"river"});
        args.as_object_mut()
            .expect("args")
            .extend(extra.as_object().expect("extra").clone());
        assert_eq!(
            fixture.call(&server, "video_create", args).await?["isError"],
            true
        );
    }
    assert!(
        !fixture
            .requests
            .lock()
            .expect("requests")
            .iter()
            .any(|r| r.method == "POST")
    );
    for prompt in ["redirect", "reject"] {
        assert_eq!(
            fixture
                .call(
                    &server,
                    "video_create",
                    json!({"model":OR_MODEL,"prompt":prompt})
                )
                .await?["isError"],
            true
        );
    }
    let mut job = json!({"id":"f-1","provider":"fal","model":FAL_MODEL,"key_id":fingerprint(KEY)});
    for (k, v) in [
        ("key_id", "different"),
        ("model", "../hub/account"),
        ("id", "../admins"),
    ] {
        let mut invalid = job.clone();
        invalid[k] = json!(v);
        assert_eq!(
            fixture
                .call(&server, "video_result", json!({"job":invalid}))
                .await?["isError"],
            true
        );
    }
    job["id"] = json!("bad-link");
    assert_eq!(
        fixture
            .call(&server, "video_result", json!({"job":job}))
            .await?["isError"],
        true
    );
    let records = fixture.requests.lock().expect("requests");
    assert_eq!(records.iter().filter(|r| r.method == "POST").count(), 2);
    assert!(
        !records
            .iter()
            .any(|r| r.path.contains("forbidden") || r.path.contains("admins"))
    );
    Ok(())
}

#[test]
fn claude_headers_rotate_with_the_listener_and_are_revoked_on_disable() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let headers = temp.path().join("wizard/video-mcp-headers.json");
    let sync = |next| install::sync_headers(&headers, next);
    for grant in ["first-test-grant", "second-test-grant"] {
        sync(Some(grant))?;
        let published: Value = serde_json::from_slice(&std::fs::read(&headers)?)?;
        assert_eq!(
            published,
            json!({"Authorization": format!("Bearer {grant}")})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&headers)?.permissions().mode() & 0o777,
                0o600
            );
        }
    }
    sync(None)?;
    assert!(!headers.exists());
    Ok(())
}

#[test]
fn install_upgrade_disable_and_collisions() -> Result<()> {
    use std::fs;
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("config.toml");
    let skill = dir.path().join("skills/coreinfra-video");
    let source = skill.join("SKILL.md");
    let original = concat!(
        "# keep this\n[shell_environment_policy.filters]\n",
        "coreinfra_api_key = 'exclude'\n\n",
        "[mcp_servers.other]\nurl = 'https://elsewhere.example'\n",
    );
    fs::write(&config, original)?;
    install::sync_at(&config, None, Some(LOCAL))?;
    let initial = fs::read_to_string(&config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(&config)?.permissions().mode() & 0o077, 0);
    }
    assert!(initial.starts_with(original));
    assert!(!initial.contains("COREINFRA_API_KEY") && !initial.contains(KEY));
    let bundled = fs::read_to_string(&source)?;
    fs::write(skill.join("my-notes"), "custom")?;
    install::sync_at(&config, Some(LOCAL), Some(LOCAL))?;
    assert_eq!(fs::read_to_string(&config)?, initial);
    let manifest = skill.join(".wizard-managed.json");
    let mut hashes: Value = serde_json::from_str(&fs::read_to_string(&manifest)?)?;
    fs::write(&source, "old bundled file")?;
    hashes["SKILL.md"] = json!(fingerprint("old bundled file"));
    fs::write(manifest, hashes.to_string())?;
    install::sync_at(&config, Some(LOCAL), Some(LOCAL))?;
    assert_eq!(fs::read_to_string(&source)?, bundled);
    fs::write(&source, "user changes")?;
    assert!(install::sync_at(&config, Some(LOCAL), Some(LOCAL)).is_err());
    install::sync_at(&config, Some(LOCAL), None)?;
    assert_eq!(fs::read_to_string(&source)?, "user changes");
    assert!(skill.join("my-notes").exists());
    assert_eq!(fs::read_to_string(&config)?, original);
    let other_dir = tempfile::tempdir()?;
    let config = other_dir.path().join("config.toml");
    let foreign = "[mcp_servers.coreinfra_media]\nurl='https://other.example'\n";
    fs::write(&config, foreign)?;
    assert!(install::sync_at(&config, None, Some(LOCAL)).is_err());
    assert_eq!(fs::read_to_string(config)?, foreign);
    assert!(!other_dir.path().join("skills").exists());
    let fresh = tempfile::tempdir()?;
    let config = fresh.path().join("config.toml");
    install::sync_at(&config, None, Some(LOCAL))?;
    assert!(!fs::read_to_string(&config)?.contains("shell_environment_policy"));
    fs::write(
        fresh
            .path()
            .join("skills/coreinfra-video/.wizard-managed.json"),
        "invalid",
    )?;
    assert!(install::sync_at(&config, Some(LOCAL), None).is_err());
    assert!(!fs::read_to_string(config)?.contains(LOCAL));
    Ok(())
}

#[tokio::test]
async fn rotated_connection_and_interrupted_download_fail_closed() -> Result<()> {
    let fixture = Fixture::new().await?;
    let server = fixture.start(Some(key("replacement-key"))).await?;
    let original = json!({"id":"f-1","provider":"fal","model":FAL_MODEL,"key_id":fingerprint(KEY)});
    let response = fixture
        .call(&server, "video_result", json!({"job":original}))
        .await?;
    assert_eq!(response["isError"], true);
    assert!(fixture.requests.lock().expect("requests").is_empty());

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let broken = tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        let (mut stream, _) = listener.accept().await?;
        let mut buf = [0u8; 1024];
        let count = stream.read(&mut buf).await?;
        assert!(count > 0);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 1000\r\n\r\npartial").await?;
        stream.shutdown().await
    });
    let response = fixture
        .hub
        .client
        .get(format!("http://{addr}/content"))
        .send()
        .await?;
    let dir = fixture.dir.path().join("truncated-download");
    assert!(save_video(&dir, response).await.is_err());
    broken.await??;
    assert_eq!(std::fs::read_dir(dir)?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn installation_and_catalog_work_without_hub_credentials() -> Result<()> {
    let fixture = Fixture::new().await?;
    let server = fixture.start(None).await?;
    assert!(
        fixture
            .dir
            .path()
            .join("profile/skills/coreinfra-video/SKILL.md")
            .is_file()
    );
    let catalog = fixture.call(&server, "video_models", json!({})).await?;
    assert_eq!(
        catalog["structuredContent"]["models"]
            .as_array()
            .context("models")?
            .len(),
        2
    );
    let missing = fixture
        .call(
            &server,
            "video_create",
            json!({"model":"unknown","prompt":"river"}),
        )
        .await?;
    assert_eq!(missing["isError"], true);
    let generation = fixture
        .call(
            &server,
            "video_create",
            json!({"model":OR_MODEL,"prompt":"river"}),
        )
        .await?;
    assert_eq!(generation["isError"], true);
    assert!(
        fixture
            .requests
            .lock()
            .expect("requests")
            .iter()
            .all(|r| r.method == "GET" && r.auth.is_none())
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires WIZARD_VIDEO_TEST_CODEX, Python and access to the public Hub catalog"]
async fn codex_skill_lists_public_media_models() -> Result<()> {
    let _ = crate::hub::install_crypto_provider();
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("codex");
    let config = profile.join("config.toml");
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/mcp", listener.local_addr()?);
    let server = server::start(listener, MediaHub::new()?, None, move |bearer| {
        install::sync_at(&config, None, Some(bearer))?;
        // Verify the production registration, then point this isolated profile at its own listener.
        let mut doc = toml::read(&config)?;
        let key = ["mcp_servers", "coreinfra_media", "url"];
        ensure!(
            toml::get_string(&doc, &key) == Some(URL),
            "Wrong installed media endpoint"
        );
        toml::set_value(&mut doc, &key, url)?;
        std::fs::write(config, doc.to_string())?;
        Ok(())
    })
    .await?;
    let python = std::env::var_os("WIZARD_VIDEO_TEST_PYTHON")
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media_catalog_e2e.py");
    let mut client = tokio::process::Command::new(python)
        .args(["-E", "-B"])
        .arg(script)
        .arg(&profile)
        .current_dir(dir.path())
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(120), client.wait()).await??;
    server.abort();
    let _ = server.await;
    ensure!(
        status.success(),
        "Codex skill/catalog E2E failed ({status})"
    );
    Ok(())
}

#[tokio::test]
async fn failed_registration_releases_the_listener_and_preserves_a_foreign_server() -> Result<()> {
    let fixture = Fixture::new().await?;
    let config = fixture.dir.path().join("config.toml");
    let foreign = "[mcp_servers.coreinfra_media]\nurl = 'https://other.example'\n";
    std::fs::write(&config, foreign)?;
    let register_path = config.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let result = server::start(
        listener,
        fixture.hub.clone(),
        Some(key(KEY)),
        move |bearer| install::sync_at(&register_path, None, Some(bearer)),
    )
    .await;
    assert!(result.is_err());
    let _new_listener = TcpListener::bind(addr).await?;
    assert_eq!(std::fs::read_to_string(config)?, foreign);
    assert!(!fixture.dir.path().join("skills").exists());
    Ok(())
}

#[tokio::test]
#[ignore = "requires WIZARD_VIDEO_TEST_PYTHON with mcp and WIZARD_VIDEO_TEST_CODEX pointing to a Codex app-server binary"]
async fn native_and_python_clients() -> Result<()> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt as _, BufReader};
    let fixture = Fixture::new().await?;
    let profile = fixture.dir.path().join("profile");
    let path = profile.join("config.toml");
    std::fs::create_dir(&profile)?;
    let dotenv = "COREINFRA_API_KEY=unrelated-codex-fixture-key\n";
    std::fs::write(profile.join(".env"), dotenv)?;
    std::fs::write(
        &path,
        "[shell_environment_policy]\nexclude = ['WIZARD_VIDEO_TEST_FILTER', 'COREINFRA_API_KEY']\n",
    )?;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/media_clients.py");
    let mut clients = tokio::process::Command::new(std::env::var("WIZARD_VIDEO_TEST_PYTHON")?)
        .arg(script)
        .arg(&profile)
        .current_dir(fixture.dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = BufReader::new(clients.stdout.take().context("client output")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(60), output.next_line()).await??;
    ensure!(
        ready.as_deref() == Some("codex-ready"),
        "Codex did not start"
    );

    // Install while Codex is running. Only the test listener's port is ephemeral.
    let server = fixture.start(Some(key(KEY))).await?;
    let config = std::fs::read_to_string(&path)?;
    assert!(config.contains(URL) && !config.contains(KEY));
    std::fs::write(&path, config.replace(URL, &server.url))?;
    clients
        .stdin
        .take()
        .context("client input")?
        .write_all(b"installed\n")
        .await?;
    let status = tokio::time::timeout(Duration::from_secs(90), clients.wait()).await??;
    ensure!(status.success(), "external client test failed");
    assert_eq!(std::fs::read_to_string(profile.join(".env"))?, dotenv);
    let requests = fixture.requests.lock().expect("requests");
    let video_bodies: Vec<_> = requests
        .iter()
        .filter(|r| {
            r.method == "POST" && r.path != audio::SPEECH && r.path != audio::TRANSCRIPTIONS
        })
        .map(|r| &r.body)
        .collect();
    assert_eq!(video_bodies.len(), 2);
    assert_eq!(
        video_bodies[0],
        &json!({"model":OR_MODEL,"prompt":"native client","provider":{"options":{"test":{"seed":0,"flag":false,"values":[null,2.75]}}}})
    );
    assert_eq!(
        video_bodies[1],
        &json!({"prompt":"python client","image_url":"data:image/png;base64,AAAA","extra":[false,null,3]})
    );
    assert!(requests.iter().any(|r| r.method == "POST"
        && r.path == audio::SPEECH
        && r.body["input"] == "python speech"));
    assert!(requests.iter().any(|r| r.method == "POST"
        && r.path == audio::TRANSCRIPTIONS
        && r.body["model"] == STT_MODEL));
    Ok(())
}

#[tokio::test]
#[ignore = "contacts the public read-only CoreInfra Hub price catalog"]
async fn public_media_catalog() -> Result<()> {
    let _ = crate::hub::install_crypto_provider();
    let models = MediaHub::new()?.models().await?;
    ensure!(!models.is_empty(), "Hub returned no video models");
    let fal = models
        .values()
        .filter(|m| m.provider == Provider::Fal)
        .count();
    println!(
        "Live public catalog: {} OpenRouter and {fal} fal video models",
        models.len() - fal
    );
    let hub = MediaHub::new()?;
    let speech = hub
        .speech_models(serde_json::from_value(json!({}))?)
        .await?;
    let speech = speech["models"].as_array().context("speech models")?;
    ensure!(!speech.is_empty(), "Hub returned no speech models");
    let first = speech[0]["id"].as_str().context("speech model ID")?;
    let detail = hub
        .speech_models(serde_json::from_value(json!({"model":first}))?)
        .await?;
    ensure!(
        detail["model"]["id"] == first,
        "Speech detail ID did not match {first}"
    );
    let params = detail["model"]["supported_parameters"]
        .as_object()
        .context("Hub returned no speech parameters")?;
    ensure!(
        !params.is_empty(),
        "Hub returned empty speech parameters for {first}"
    );
    let transcription = hub.transcription_models().await?;
    let transcription = transcription["models"]
        .as_array()
        .context("transcription models")?;
    ensure!(
        !transcription.is_empty(),
        "Hub returned no transcription models"
    );
    println!(
        "Live public catalog: {} speech and {} transcription models",
        speech.len(),
        transcription.len()
    );
    Ok(())
}

fn extension(saved: &Value) -> Option<&str> {
    Path::new(saved["path"].as_str()?).extension()?.to_str()
}

fn posts(fixture: &Fixture, path: &str) -> Vec<Value> {
    fixture
        .requests
        .lock()
        .expect("requests")
        .iter()
        .filter(|r| r.method == "POST" && r.path == path)
        .map(|r| r.body.clone())
        .collect()
}

#[test]
fn downloads_share_one_media_directory() -> Result<()> {
    let hub = MediaHub::new()?;
    let dir = hub.output_dir.context("Downloads directory")?;
    assert!(dir.ends_with("CoreInfra Media"));
    Ok(())
}

#[tokio::test]
async fn speech_catalog_joins_prices_and_details_validate_the_model() -> Result<()> {
    let fixture = Fixture::new().await?;
    let server = fixture.start(None).await?;
    let tools = fixture
        .request(&server.url, "tools/list", &json!({}))
        .bearer_auth(&server.bearer)
        .send()
        .await?;
    let tools: Value = serde_json::from_slice(&tools.bytes().await?)?;
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .context("tools")?
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "video_models",
            "video_create",
            "video_result",
            "speech_models",
            "speech_create",
            "transcription_models",
            "transcription_create"
        ]
    );

    let list =
        fixture.call(&server, "speech_models", json!({})).await?["structuredContent"].clone();
    assert_eq!(
        list["models"][0],
        json!({"id":TTS_MODEL,"name":"Example: Voice","response_formats":["mp3","pcm"],
            "voice_count":3,"speed":{"min":0.5,"max":2},"parameters":["response_format","speed","voice"],
            "min_rub_per_1k_characters":2.5,"max_rub_per_1k_characters":5.0})
    );
    assert_eq!(list["models"][1]["id"], "example/unpriced");
    assert!(list["models"][1]["max_rub_per_1k_characters"].is_null());
    // The compact list leaves voice names to the per-model detail.
    assert!(!list.to_string().contains("alice"));

    let detail = fixture
        .call(&server, "speech_models", json!({"model":TTS_MODEL}))
        .await?["structuredContent"]["model"]
        .clone();
    assert_eq!(detail["id"], TTS_MODEL);
    assert_eq!(detail["description"], "Fixture speech model");
    assert_eq!(
        detail["input_modalities"],
        json!([{"type":"text","turns":"optional"}])
    );
    assert_eq!(
        detail["supported_parameters"]["voice"]["values"],
        json!(["alice", "bob", "carol"])
    );
    assert_eq!(detail["max_rub_per_1k_characters"], 5.0);

    let before = fixture.requests.lock().expect("requests").len();
    for model in [
        "../hub",
        "example",
        "example/voice/extra",
        "example/..",
        "/voice",
        "a b/c",
    ] {
        let invalid = fixture
            .call(&server, "speech_models", json!({"model":model}))
            .await?;
        assert_eq!(invalid["isError"], true, "{model}");
    }
    assert_eq!(fixture.requests.lock().expect("requests").len(), before);
    Ok(())
}

#[tokio::test]
async fn speech_is_saved_without_overwriting_and_never_streams() -> Result<()> {
    let fixture = Fixture::new().await?;
    let server = fixture.start(Some(key(KEY))).await?;
    let downloads = fixture.dir.path().join("downloads");
    let args = json!({"model":TTS_MODEL,"input":"Привет","voice":"alice","speed":1.25,
        "fields":{"instructions":"calm","provider":{"options":{"coreinfra":{"seed":0}}}}});
    let first =
        fixture.call(&server, "speech_create", args.clone()).await?["structuredContent"].clone();
    let second = fixture.call(&server, "speech_create", args).await?["structuredContent"].clone();
    for saved in [&first, &second] {
        let path = Path::new(saved["path"].as_str().context("speech path")?);
        assert!(path.starts_with(&downloads));
        let name = path.file_name().and_then(|n| n.to_str()).context("name")?;
        assert!(
            name.starts_with("speech-") && extension(saved) == Some("mp3"),
            "{name}"
        );
        assert_eq!(std::fs::read(path)?, MP3);
        assert_eq!(saved["content_type"], "audio/mpeg");
        assert_eq!(saved["size_bytes"], MP3.len());
    }
    assert_ne!(first["path"], second["path"]);
    let pcm = fixture
        .call(
            &server,
            "speech_create",
            json!({"model":TTS_MODEL,"input":"raw","response_format":"pcm"}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(extension(&pcm), Some("pcm"));
    assert_eq!(pcm["content_type"], "audio/pcm");
    let octet = fixture
        .call(
            &server,
            "speech_create",
            json!({"model":TTS_MODEL,"input":"octet","response_format":"mp3"}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(extension(&octet), Some("mp3"));
    // Unknown caller format names never become file extensions on binary responses.
    let unknown_ext = fixture
        .call(
            &server,
            "speech_create",
            json!({"model":TTS_MODEL,"input":"octet","response_format":"exe"}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(extension(&unknown_ext), Some("bin"));
    // Multi-speaker turns serialize into the upstream `input` array.
    let dialogue = fixture
        .call(
            &server,
            "speech_create",
            json!({"model":TTS_MODEL,"turns":[
                {"text":"Привет","voice":"alice","instructions":"warm"},
                {"text":"Пока","voice":"bob"}
            ]}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(extension(&dialogue), Some("mp3"));
    let bodies = posts(&fixture, audio::SPEECH);
    assert_eq!(
        bodies[0],
        json!({"instructions":"calm","provider":{"options":{"coreinfra":{"seed":0}}},
            "model":TTS_MODEL,"input":"Привет","voice":"alice","speed":1.25})
    );
    assert_eq!(
        bodies.last(),
        Some(&json!({"model":TTS_MODEL,"input":[
            {"text":"Привет","voice":"alice","instructions":"warm"},
            {"text":"Пока","voice":"bob"}
        ]}))
    );
    assert!(
        fixture
            .requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|r| r.path == audio::SPEECH)
            .all(|r| r.auth.as_deref() == Some("Bearer fixture-upstream-secret"))
    );

    let submitted = bodies.len();
    for invalid in [
        json!({"model":TTS_MODEL,"input":"x","fields":{"stream":true}}),
        json!({"model":TTS_MODEL,"input":"x","fields_json":"{\"stream_format\":\"sse\"}"}),
        json!({"model":TTS_MODEL,"input":"x","fields":{"voice":"bob"}}),
        json!({"model":TTS_MODEL,"input":"x","fields":{"model":"other/model"}}),
        json!({"model":TTS_MODEL,"input":"x","fields":{},"fields_json":"{}"}),
        json!({"model":TTS_MODEL,"input":"  "}),
        json!({"model":TTS_MODEL}),
        json!({"model":TTS_MODEL,"input":"x","turns":[{"text":"y","voice":"alice"}]}),
        json!({"model":TTS_MODEL,"turns":[]}),
        json!({"model":TTS_MODEL,"turns":[{"text":" ","voice":"alice"}]}),
        json!({"model":"../hub","input":"x"}),
        json!({"model":TTS_MODEL,"input":"x","response_format":"../mp3"}),
        json!({"model":TTS_MODEL,"input":"x","speed":-1}),
        json!({"model":TTS_MODEL,"input":"x","path":"/tmp/out.mp3"}),
    ] {
        let response = fixture
            .call(&server, "speech_create", invalid.clone())
            .await?;
        assert_eq!(response["isError"], true, "{invalid}");
    }
    assert_eq!(posts(&fixture, audio::SPEECH).len(), submitted);

    let files = std::fs::read_dir(&downloads)?.count();
    for input in ["html", "json", "empty"] {
        let response = fixture
            .call(
                &server,
                "speech_create",
                json!({"model":TTS_MODEL,"input":input}),
            )
            .await?;
        assert_eq!(response["isError"], true, "{input}");
    }
    // Rejected responses leave neither audio nor partial files behind.
    assert_eq!(std::fs::read_dir(&downloads)?.count(), files);

    let unkeyed = fixture.start(None).await?;
    let missing = fixture
        .call(
            &unkeyed,
            "speech_create",
            json!({"model":TTS_MODEL,"input":"x"}),
        )
        .await?;
    assert_eq!(missing["isError"], true);
    assert_eq!(posts(&fixture, audio::SPEECH).len(), submitted + 3);
    Ok(())
}

#[tokio::test]
async fn transcription_sends_inline_audio_and_returns_json() -> Result<()> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let fixture = Fixture::new().await?;
    let server = fixture.start(Some(key(KEY))).await?;
    let models = fixture
        .call(&server, "transcription_models", json!({}))
        .await?["structuredContent"]["models"]
        .clone();
    assert_eq!(
        models,
        json!([{"id":STT_MODEL,"name":"Scribe","min_rub_per_minute":0.6,"max_rub_per_minute":0.6}])
    );

    let audio = STANDARD.encode(b"RIFF fixture wav");
    let result = fixture
        .call(
            &server,
            "transcription_create",
            json!({"model":STT_MODEL,"audio_base64":audio,"format":"WAV","language":"ru",
                "prompt":"CoreInfra","fields_json":"{\"temperature\":0}"}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(result["text"], "привет");
    assert_eq!(result["usage"], json!({"cost_rub":0.25,"seconds":3}));
    assert_eq!(result["cost_rub"], 0.25);
    let verbose = fixture
        .call(
            &server,
            "transcription_create",
            json!({"model":STT_MODEL,"audio_base64":format!("data:audio/mpeg;base64,{audio}"),
                "format":"mp3","response_format":"verbose_json",
                "fields":{"provider":{"only":["coreinfra"]}}}),
        )
        .await?["structuredContent"]
        .clone();
    assert_eq!(verbose["text"], "привет");
    assert_eq!(
        verbose["segments"],
        json!([{"start":0.0,"end":1.0,"text":"привет"}])
    );

    let bodies = posts(&fixture, audio::TRANSCRIPTIONS);
    assert_eq!(
        bodies[0],
        json!({"temperature":0,"model":STT_MODEL,"input_audio":{"data":audio,"format":"wav"},
            "language":"ru","prompt":"CoreInfra"})
    );
    assert_eq!(
        bodies[1]["input_audio"],
        json!({"data":audio,"format":"mp3"})
    );
    assert_eq!(bodies[1]["provider"], json!({"only":["coreinfra"]}));

    for invalid in [
        json!({"model":STT_MODEL,"audio_base64":"not base64!","format":"wav"}),
        json!({"model":STT_MODEL,"audio_base64":"","format":"wav"}),
        json!({"model":STT_MODEL,"audio_base64":audio,"format":"../wav"}),
        json!({"model":STT_MODEL,"audio_base64":audio,"format":"wav","fields":{"input_audio":{}}}),
        json!({"model":STT_MODEL,"audio_base64":audio,"format":"wav","fields":{"stream":true}}),
        json!({"model":STT_MODEL,"audio_base64":audio,"format":"wav","fields_json":"[]"}),
        json!({"model":STT_MODEL,"path":"/etc/passwd","format":"wav"}),
        json!({"model":"whisper","audio_base64":audio,"format":"wav"}),
    ] {
        let response = fixture
            .call(&server, "transcription_create", invalid.clone())
            .await?;
        assert_eq!(response["isError"], true, "{invalid}");
    }
    assert_eq!(posts(&fixture, audio::TRANSCRIPTIONS).len(), 2);
    Ok(())
}

#[test]
fn codex_upgrade_from_1_0_10_preserves_restrictions_and_foreign_servers() -> Result<()> {
    use std::fs;
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("config.toml");
    let bearer = format!("wizard-video-{}", uuid::Uuid::new_v4());
    let legacy = format!(
        "model = 'gpt'\n\n[mcp_servers.coreinfra_video]\nurl = '{URL}'\nenabled = false\ndisabled_tools = ['video_create']\ntool_timeout_sec = 600\n\n[mcp_servers.coreinfra_video.http_headers]\nAuthorization = 'Bearer {bearer}'\n"
    );
    fs::write(&config, &legacy)?;
    let video = dir.path().join("skills/coreinfra-video");
    fs::create_dir_all(&video)?;
    fs::write(video.join("SKILL.md"), "1.0.10 skill")?;
    fs::write(
        video.join(".wizard-managed.json"),
        json!({"SKILL.md": fingerprint("1.0.10 skill")}).to_string(),
    )?;
    assert!(install::needs_apply_at(&config, Some(&bearer)));
    install::sync_at(&config, Some(&bearer), Some(&bearer))?;
    assert!(!install::needs_apply_at(&config, Some(&bearer)));
    let doc = toml::read(&config)?;
    assert!(doc["mcp_servers"].get("coreinfra_video").is_none());
    assert_eq!(
        toml::get_string(&doc, &["mcp_servers", "coreinfra_media", "url"]),
        Some(URL)
    );
    assert_eq!(
        doc["mcp_servers"]["coreinfra_media"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        doc["mcp_servers"]["coreinfra_media"]["disabled_tools"]
            .as_array()
            .and_then(|a| a.get(0))
            .and_then(toml_edit::Value::as_str),
        Some("video_create")
    );
    assert_eq!(
        doc["mcp_servers"]["coreinfra_media"]["tool_timeout_sec"].as_integer(),
        Some(600)
    );
    assert_eq!(
        toml::get_string(
            &doc,
            &[
                "mcp_servers",
                "coreinfra_media",
                "http_headers",
                "Authorization"
            ]
        ),
        Some(format!("Bearer {bearer}").as_str())
    );
    for skill in [
        "coreinfra-video",
        "coreinfra-speech",
        "coreinfra-transcription",
    ] {
        let root = dir.path().join("skills").join(skill);
        let text = fs::read_to_string(root.join("SKILL.md"))?;
        assert!(text.contains("coreinfra_media"), "{skill}");
        assert!(root.join("agents/openai.yaml").is_file());
        assert!(root.join(".wizard-managed.json").is_file());
    }
    install::sync_at(&config, Some(&bearer), None)?;
    assert_eq!(fs::read_to_string(&config)?, "model = 'gpt'\n");
    assert!(!dir.path().join("skills/coreinfra-speech/SKILL.md").exists());

    // Disabling with a foreign coreinfra_media still cleans up an owned 1.0.10 entry.
    let mixed = format!(
        "[mcp_servers.coreinfra_media]\nurl = 'https://other.example/mcp'\n\n[mcp_servers.coreinfra_video]\nurl = '{URL}'\n\n[mcp_servers.coreinfra_video.http_headers]\nAuthorization = 'Bearer {bearer}'\n"
    );
    fs::write(&config, &mixed)?;
    assert!(install::sync_at(&config, Some(&bearer), Some(&bearer)).is_err());
    install::sync_at(&config, Some(&bearer), None)?;
    let doc = toml::read(&config)?;
    assert!(doc["mcp_servers"].get("coreinfra_video").is_none());
    assert_eq!(
        toml::get_string(&doc, &["mcp_servers", "coreinfra_media", "url"]),
        Some("https://other.example/mcp")
    );

    // A server under the old name that Wizard did not register is never removed.
    let foreign = format!(
        "[mcp_servers.coreinfra_video]\nurl = 'https://other.example/mcp'\n\n[mcp_servers.coreinfra_video.http_headers]\nAuthorization = 'Bearer {bearer}'\n"
    );
    fs::write(&config, &foreign)?;
    install::sync_at(&config, None, Some(LOCAL))?;
    install::sync_at(&config, Some(LOCAL), None)?;
    assert_eq!(fs::read_to_string(&config)?, foreign);
    Ok(())
}
