use super::*;
use crate::{config_files::toml, video::server::VideoAccess};
use alloc::sync::Arc;
use core::{
    convert::Infallible,
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};
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

struct Access(&'static str);
impl VideoAccess for Access {
    fn token<'a>(
        &'a self,
        _: Option<&'a Job>,
    ) -> impl Future<Output = Result<VideoKey>> + Send + 'a {
        core::future::ready(Ok(VideoKey {
            hub_id: "team".into(),
            token: self.0.into(),
        }))
    }
}

#[derive(Default, Clone)]
struct SignedOut {
    lookups: Arc<AtomicUsize>,
}

impl VideoAccess for SignedOut {
    fn token<'a>(
        &'a self,
        _: Option<&'a Job>,
    ) -> impl Future<Output = Result<VideoKey>> + Send + 'a {
        core::future::poll_fn(|_| {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            core::task::Poll::Ready(Err(anyhow!("Sign in to Wizard")))
        })
    }
}

struct Seen {
    method: String,
    path: String,
    auth: Option<String>,
    body: Value,
}

struct Fixture {
    hub: VideoHub,
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
        "ignored":{"category":"text","endpoints":["/text"],"prices":{}}
    }}}})
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
        let mut hub = VideoHub::new()?;
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
                            let answer = if path == "/hub/api/prices" && auth.is_none() {
                                catalog()
                            } else if path.ends_with("/content?t=fixture") && auth.is_none()
                                || path.ends_with("/content?index=1")
                                    && auth.as_deref() == Some("Bearer fixture-upstream-secret")
                            {
                                return Ok::<_, Infallible>(HttpResponse::new(Full::new(
                                    Bytes::from_static(MP4),
                                )));
                            } else if auth.as_deref() != Some("Bearer fixture-upstream-secret") {
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

    async fn start(&self, access: impl VideoAccess) -> Result<RunningServer> {
        let path = self.dir.path().join("profile/config.toml");
        let config = path.clone();
        let local = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/mcp", local.local_addr()?);
        let task = server::start(local, self.hub.clone(), access, move |bearer| {
            install::sync_at(&config, None, Some(bearer))
        })
        .await?;
        let doc = toml::read(&path)?;
        let bearer = toml::get_string(
            &doc,
            &[
                "mcp_servers",
                "coreinfra_video",
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
async fn both_providers_fields_and_resume_across_restart() -> Result<()> {
    let fixture = Fixture::new().await?;
    let first = fixture.start(Access(KEY)).await?;
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
    let second = fixture.start(Access(KEY)).await?;
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
    let server = fixture.start(Access(KEY)).await?;
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
    let mut job = json!({"id":"f-1","provider":"fal","model":FAL_MODEL,"hub_id":"team","key_id":fingerprint(KEY)});
    for (k, v) in [
        ("hub_id", "another"),
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
    let foreign = "[mcp_servers.coreinfra_video]\nurl='https://other.example'\n";
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
    let server = fixture.start(Access("replacement-key")).await?;
    let original = json!({"id":"f-1","provider":"fal","model":FAL_MODEL,"hub_id":"team","key_id":fingerprint(KEY)});
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
    let access = SignedOut::default();
    let server = fixture.start(access.clone()).await?;
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
    assert_eq!(access.lookups.load(Ordering::SeqCst), 0);
    let generation = fixture
        .call(
            &server,
            "video_create",
            json!({"model":OR_MODEL,"prompt":"river"}),
        )
        .await?;
    assert_eq!(generation["isError"], true);
    assert_eq!(access.lookups.load(Ordering::SeqCst), 1);
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
async fn codex_skill_lists_public_video_models() -> Result<()> {
    let _ = crate::hub::install_crypto_provider();
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("codex");
    let config = profile.join("config.toml");
    let access = SignedOut::default();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/mcp", listener.local_addr()?);
    let server = server::start(listener, VideoHub::new()?, access.clone(), move |bearer| {
        install::sync_at(&config, None, Some(bearer))?;
        // Verify the production registration, then point this isolated profile at its own listener.
        let mut doc = toml::read(&config)?;
        let key = ["mcp_servers", "coreinfra_video", "url"];
        ensure!(
            toml::get_string(&doc, &key) == Some(URL),
            "Wrong installed video endpoint"
        );
        toml::set_value(&mut doc, &key, url)?;
        std::fs::write(config, doc.to_string())?;
        Ok(())
    })
    .await?;
    let python = std::env::var_os("WIZARD_VIDEO_TEST_PYTHON")
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/video_catalog_e2e.py");
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
    assert_eq!(
        access.lookups.load(Ordering::SeqCst),
        0,
        "catalog requested a Hub key"
    );
    Ok(())
}

#[tokio::test]
async fn failed_registration_releases_the_listener_and_preserves_a_foreign_server() -> Result<()> {
    let fixture = Fixture::new().await?;
    let config = fixture.dir.path().join("config.toml");
    let foreign = "[mcp_servers.coreinfra_video]\nurl = 'https://other.example'\n";
    std::fs::write(&config, foreign)?;
    let register_path = config.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let result = server::start(listener, fixture.hub.clone(), Access(KEY), move |bearer| {
        install::sync_at(&register_path, None, Some(bearer))
    })
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
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/video_clients.py");
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
    let server = fixture.start(Access(KEY)).await?;
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
    let bodies: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "POST")
        .map(|r| &r.body)
        .collect();
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        bodies[0],
        &json!({"model":OR_MODEL,"prompt":"native client","provider":{"options":{"test":{"seed":0,"flag":false,"values":[null,2.75]}}}})
    );
    assert_eq!(
        bodies[1],
        &json!({"prompt":"python client","image_url":"data:image/png;base64,AAAA","extra":[false,null,3]})
    );
    Ok(())
}

#[tokio::test]
#[ignore = "contacts the public read-only CoreInfra Hub price catalog"]
async fn public_video_catalog() -> Result<()> {
    let _ = crate::hub::install_crypto_provider();
    let models = VideoHub::new()?.models().await?;
    ensure!(!models.is_empty(), "Hub returned no video models");
    let fal = models
        .values()
        .filter(|m| m.provider == Provider::Fal)
        .count();
    println!(
        "Live public catalog: {} OpenRouter and {fal} fal video models",
        models.len() - fal
    );
    Ok(())
}
