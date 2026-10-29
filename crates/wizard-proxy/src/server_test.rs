use super::*;
use core::time::Duration;
use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_rustls::rustls::RootCertStore;

fn state() -> Arc<ServerState> {
    let tls = ClientConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_root_certificates(RootCertStore::empty())
    .with_no_client_auth();
    Arc::new(ServerState::new(tls))
}

fn settings(claude: Option<&str>, chatgpt: Option<&str>) {
    wizard_settings::save_to_file(&wizard_settings::Settings {
        claude_proxy_enabled: claude.map(str::to_owned),
        codex_proxy_enabled: chatgpt.map(str::to_owned),
        ..wizard_settings::Settings::default()
    })
    .expect("save isolated settings");
}

async fn disabling_reload_replies_before_exit() -> io::Result<()> {
    settings(Some("claude-fixture"), Some("chatgpt-fixture"));
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let claude = TcpListener::bind("127.0.0.1:0").await?;
    let chatgpt = TcpListener::bind("127.0.0.1:0").await?;
    let address = chatgpt.local_addr()?;
    let task = tokio::spawn(run_listeners(
        state,
        claude,
        chatgpt,
        core::future::pending(),
    ));

    let status: control::Status =
        control::request(&address.to_string(), control::Command::Status).await?;
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
    assert!(status.claude_desktop_enabled && status.chatgpt_enabled);

    settings(None, None);
    let response: control::ReloadResponse =
        control::request(&address.to_string(), control::Command::Reload).await?;
    assert!(response.error.is_none());
    assert_eq!(response.status.version, env!("CARGO_PKG_VERSION"));
    assert!(!response.status.enabled());
    task.await.map_err(io::Error::other)??;
    Ok(())
}

async fn shutdown_replies_before_exit_and_preserves_settings() -> io::Result<()> {
    settings(Some("shutdown-fixture"), None);
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let claude = TcpListener::bind("127.0.0.1:0").await?;
    let chatgpt = TcpListener::bind("127.0.0.1:0").await?;
    let address = claude.local_addr()?;
    let task = tokio::spawn(run_listeners(
        state,
        claude,
        chatgpt,
        core::future::pending(),
    ));

    let destination = TcpListener::bind("127.0.0.1:0").await?;
    let target = destination.local_addr()?;
    let mut client = TcpStream::connect(address).await?;
    client
        .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await?;
    let (mut remote, _) = destination.accept().await?;

    let response: control::ShutdownResponse =
        control::request(&address.to_string(), control::Command::Shutdown).await?;
    assert_eq!(response.pid, std::process::id());
    task.await.map_err(io::Error::other)??;
    // A completed Shutdown closes both ends of an active direct tunnel.
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await?;
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    assert_eq!(remote.read(&mut [0_u8; 1]).await?, 0);
    assert!(
        wizard_settings::load_from_file()
            .expect("saved settings")
            .local_proxy_enabled()
    );
    Ok(())
}

async fn accepted_reload_survives_disconnect_and_shutdown() -> io::Result<()> {
    settings(Some("initial"), None);
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let old = state
        .claude_desktop
        .current
        .read()
        .await
        .clone()
        .expect("enabled slot");
    // Hold draining so disconnect and shutdown occur during an accepted Reload.
    let active = old.active.read().await;
    let mut cancelled = old.cancelled.subscribe();
    let claude = TcpListener::bind("127.0.0.1:0").await?;
    let chatgpt = TcpListener::bind("127.0.0.1:0").await?;
    let address = claude.local_addr()?;
    let (stop, shutdown) = oneshot::channel();
    let mut task = tokio::spawn(run_listeners(Arc::clone(&state), claude, chatgpt, async {
        let _ = shutdown.await;
    }));

    settings(Some("replacement"), None);
    let mut client = TcpStream::connect(address).await?;
    client
        .write_all(
            format!("POST /reload HTTP/1.1\r\nHost: {address}\r\nContent-Length: 0\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    cancelled.changed().await.map_err(io::Error::other)?;
    drop(client);
    let _ = stop.send(());
    assert!(
        timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err()
    );

    drop(active);
    task.await.map_err(io::Error::other)??;
    assert!(
        state
            .claude_desktop
            .current
            .read()
            .await
            .as_ref()
            .is_some_and(|generation| generation.token == "replacement")
    );
    Ok(())
}

// Windows Known Folder resolution ignores HOME; never use a real user profile.
// The test and all shared protocol code still compile on Windows.
#[test]
#[cfg_attr(
    windows,
    ignore = "requires isolated Unix HOME/XDG settings resolution"
)]
fn isolated_http_management() {
    const MARKER: &str = "WIZARD_HTTP_TEST_HOME";
    if let Some(home) = std::env::var_os(MARKER) {
        assert_eq!(std::env::var_os("HOME"), Some(home.clone()));
        let path = wizard_settings::paths::settings_path().expect("settings path");
        assert!(
            path.starts_with(Path::new(&home)),
            "refuse real settings path"
        );
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                timeout(Duration::from_secs(30), async {
                    disabling_reload_replies_before_exit()
                        .await
                        .expect("disabling Reload response");
                    shutdown_replies_before_exit_and_preserves_settings()
                        .await
                        .expect("graceful Shutdown response");
                    accepted_reload_survives_disconnect_and_shutdown()
                        .await
                        .expect("shutdown during disconnected Reload");
                })
                .await
                .expect("management fixtures timed out");
            });
        return;
    }
    let directory = std::env::temp_dir().join(format!(
        "wizard-http-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir(&directory).expect("fixture directory");
    let result = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "server_tests::isolated_http_management"])
        .env(MARKER, &directory)
        .env("HOME", &directory)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .output()
        .expect("isolated test process");
    fs::remove_dir_all(directory).expect("fixture cleanup");
    assert!(
        result.status.success(),
        "isolated management test failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
