use super::*;
use crate::video::{VideoHub, install, server};
use core::time::Duration;
use tokio::{net::TcpListener, process::Command};

#[tokio::test]
#[ignore = "requires native Claude Code, Python and the public Hub catalog"]
async fn claude_code_skill_lists_public_video_models() -> Result<()> {
    let _ = crate::hub::install_crypto_provider();
    let temp = tempfile::tempdir()?;
    let root = std::path::absolute(temp.path())?;
    let profile = root.join("claude config");
    let global = profile.join(".claude.json");
    let headers = root.join("wizard's app data & ${HOME}/video-mcp-headers.json");
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/mcp", listener.local_addr()?);
    let registration = video::server(&url, &headers)?;
    let broker = server::start(listener, VideoHub::new()?, None, move |bearer| {
        install::sync_headers(&headers, Some(bearer))
    })
    .await?;
    prepare_routing_at(
        &profile,
        &global,
        &registration,
        RoutingMode::Hub,
        "fixture-inference-key",
        true,
    )?
    .apply_with_backup(
        None,
        |_| bail!("Unexpected desktop proxy"),
        |_| bail!("Unexpected Codex proxy"),
    )?;
    let python = std::env::var_os("WIZARD_VIDEO_TEST_PYTHON")
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
    let mut child = Command::new(python)
        .args(["-E", "-B"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/claude_video_e2e.py"))
        .arg(&profile)
        .current_dir(root)
        .kill_on_drop(true)
        .spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(120), child.wait()).await??;
    broker.abort();
    ensure!(status.success(), "Claude Code video E2E failed ({status})");
    Ok(())
}
