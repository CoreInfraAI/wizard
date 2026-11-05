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

#[tokio::test]
async fn accept_errors_preserve_tunnels_and_resume_requests_after_backoff() -> io::Result<()> {
    timeout(Duration::from_secs(5), async {
        let state = state();
        state.slot.set_token(Some("fixture"), &state.tls).await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let destination = TcpListener::bind("127.0.0.1:0").await?;
        let target = destination.local_addr()?;
        let attempts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&attempts);
        let (stop, mut shutdown) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            accept_connections(
                || {
                    let attempt = {
                        let mut times = recorded.lock().expect("attempt times");
                        times.push(tokio::time::Instant::now());
                        times.len()
                    };
                    let listener = &listener;
                    async move {
                        match attempt {
                            2 => Err(io::Error::from(io::ErrorKind::OutOfMemory)),
                            3 => Err(io::Error::from(io::ErrorKind::ConnectionAborted)),
                            _ => listener.accept().await.map(|(stream, _)| stream),
                        }
                    }
                },
                &state.slot,
                Some(&state),
                &mut shutdown,
                &mut connections,
            )
            .await;
            connections.abort_all();
            while let Some(result) = connections.join_next().await {
                if let Err(error) = result {
                    assert!(
                        error.is_cancelled(),
                        "unexpected connection failure: {error}"
                    );
                }
            }
        });

        let mut client = TcpStream::connect(address).await?;
        client
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await?;
        let (mut remote, _) = destination.accept().await?;
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(client.read_u8().await?);
            assert!(header.len() < 8192);
        }
        assert!(header.starts_with(b"HTTP/1.1 200"));
        // The first accepted connection survives subsequent accept failures.
        client.write_all(b"live").await?;
        let mut bytes = [0; 4];
        remote.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"live");

        let response: control::Status =
            control::request(&address.to_string(), control::Command::Status).await?;
        assert!(response.enabled);
        {
            let times = attempts.lock().expect("attempt times");
            assert!(times.len() >= 4);
            assert!(times[2] - times[1] >= ACCEPT_RETRY_DELAY);
            assert!(times[3] - times[2] >= ACCEPT_RETRY_DELAY);
        }
        remote.write_all(b"back").await?;
        client.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"back");
        let _ = stop.send(());
        task.await.map_err(io::Error::other)?;
        assert_eq!(client.read(&mut bytes).await?, 0);
        Ok(())
    })
    .await
    .expect("accept recovery timed out")
}

#[tokio::test]
async fn shutdown_interrupts_accept_backoff() {
    let state = state();
    let (stop, mut shutdown) = oneshot::channel();
    let (failed, mut failure) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        accept_connections(
            || async {
                failed.send(()).expect("report injected failure");
                Err(io::Error::from(io::ErrorKind::OutOfMemory))
            },
            &state.slot,
            None,
            &mut shutdown,
            &mut connections,
        )
        .await;
    });
    timeout(Duration::from_secs(2), failure.recv())
        .await
        .expect("injected failure");
    let _ = stop.send(());
    timeout(Duration::from_millis(250), task)
        .await
        .expect("shutdown must not wait for the retry delay")
        .expect("accept task");
    assert!(failure.try_recv().is_err(), "must not retry in a hot loop");
}

fn settings(token: Option<&str>, claude: bool, chatgpt: bool) {
    wizard_settings::save_to_file(&wizard_settings::Settings {
        claude_proxy_enabled: claude,
        codex_proxy_enabled: chatgpt,
        proxy_token: token.map(str::to_owned),
        ..wizard_settings::Settings::default()
    })
    .expect("save isolated settings");
}

async fn disabling_reload_replies_before_exit() -> io::Result<()> {
    settings(Some("wizard-fixture"), true, true);
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
    assert!(status.enabled);

    settings(None, false, false);
    let response: control::ReloadResponse =
        control::request(&address.to_string(), control::Command::Reload).await?;
    assert!(response.error.is_none());
    assert_eq!(response.status.version, env!("CARGO_PKG_VERSION"));
    assert!(!response.status.enabled);
    task.await.map_err(io::Error::other)??;
    Ok(())
}

async fn shutdown_replies_before_exit_and_preserves_settings() -> io::Result<()> {
    settings(Some("shutdown-fixture"), true, false);
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

    // A browser page's cross-origin POST carries Origin and must not stop the proxy.
    let mut browser = TcpStream::connect(address).await?;
    browser
        .write_all(
            format!(
                "POST /shutdown HTTP/1.1\r\nHost: {address}\r\nOrigin: https://example.com\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let mut rejected = Vec::new();
    browser.read_to_end(&mut rejected).await?;
    assert!(rejected.starts_with(b"HTTP/1.1 400"));

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
    settings(Some("initial"), true, false);
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let old = state
        .slot
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

    settings(Some("replacement"), true, false);
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
            .slot
            .current
            .read()
            .await
            .as_ref()
            .is_some_and(|generation| generation.token == "replacement")
    );
    Ok(())
}

async fn shared_generation_preserves_and_drains_tunnels_on_both_ports() -> io::Result<()> {
    settings(Some("shared-a"), true, false);
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let claude = TcpListener::bind("127.0.0.1:0").await?;
    let chatgpt = TcpListener::bind("127.0.0.1:0").await?;
    let addresses = [
        claude.local_addr()?.to_string(),
        chatgpt.local_addr()?.to_string(),
    ];
    let task = tokio::spawn(run_listeners(
        Arc::clone(&state),
        claude,
        chatgpt,
        core::future::pending(),
    ));

    let destination = TcpListener::bind("127.0.0.1:0").await?;
    let target = destination.local_addr()?;
    let mut tunnels = Vec::new();
    for address in &addresses {
        let status: control::Status = control::request(address, control::Command::Status).await?;
        assert!(status.enabled);
        let mut client = TcpStream::connect(address).await?;
        client
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await?;
        let (remote, _) = destination.accept().await?;
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(client.read_u8().await?);
            assert!(header.len() < 8192);
        }
        assert!(header.starts_with(b"HTTP/1.1 200"));
        tunnels.push((client, remote));
    }

    // Switching the sole consumer does not interrupt either port's live tunnel.
    settings(Some("shared-a"), false, true);
    let switched: control::ReloadResponse =
        control::request(&addresses[0], control::Command::Reload).await?;
    assert!(switched.error.is_none() && switched.status.enabled);
    for (client, remote) in &mut tunnels {
        client.write_all(b"live").await?;
        let mut bytes = [0; 4];
        remote.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"live");
        remote.write_all(b"back").await?;
        client.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"back");
    }

    // A token change drains both ports before the management reply completes.
    settings(Some("shared-b"), false, true);
    let replaced: control::ReloadResponse =
        control::request(&addresses[1], control::Command::Reload).await?;
    assert!(replaced.error.is_none() && replaced.status.enabled);
    for (client, remote) in &mut tunnels {
        assert_eq!(client.read(&mut [0; 1]).await?, 0);
        assert_eq!(remote.read(&mut [0; 1]).await?, 0);
    }
    assert_eq!(
        state
            .slot
            .current
            .read()
            .await
            .as_ref()
            .expect("generation")
            .token,
        "shared-b"
    );

    // System demand alone keeps both managed ports enabled after agents opt out.
    wizard_settings::save_to_file(&wizard_settings::Settings {
        system_proxy_enabled: true,
        proxy_token: Some("shared-b".into()),
        ..Default::default()
    })
    .map_err(io::Error::other)?;
    let system: control::ReloadResponse =
        control::request(&addresses[1], control::Command::Reload).await?;
    assert!(system.error.is_none() && system.status.enabled);

    // Keeping the credential does not keep the daemon alive without consumers.
    settings(Some("shared-b"), false, false);
    let stopped: control::ReloadResponse =
        control::request(&addresses[0], control::Command::Reload).await?;
    assert!(stopped.error.is_none() && !stopped.status.enabled);
    task.await.map_err(io::Error::other)??;
    Ok(())
}

async fn failed_reload_keeps_working_configuration() -> io::Result<()> {
    settings(Some("working"), true, true);
    let state = state();
    assert!(state.reload_with_lock().await.error.is_none());
    let original = state.slot.current.read().await.clone().expect("generation");
    let claude = TcpListener::bind("127.0.0.1:0").await?;
    let chatgpt = TcpListener::bind("127.0.0.1:0").await?;
    let address = claude.local_addr()?.to_string();
    let (stop, shutdown) = oneshot::channel();
    let task = tokio::spawn(run_listeners(Arc::clone(&state), claude, chatgpt, async {
        let _ = shutdown.await;
    }));
    fs::write(
        wizard_settings::paths::settings_path().expect("settings path"),
        "invalid = [",
    )?;
    let failed: control::ReloadResponse =
        control::request(&address, control::Command::Reload).await?;
    assert!(failed.error.is_some());
    assert!(failed.status.enabled);

    // Invalid shared credentials must preserve the working generation.
    for token in [None, Some("")] {
        settings(token, true, true);
        let failed: control::ReloadResponse =
            control::request(&address, control::Command::Reload).await?;
        assert!(failed.error.is_some());
        assert!(failed.status.enabled);
        assert!(Arc::ptr_eq(
            &original,
            state
                .slot
                .current
                .read()
                .await
                .as_ref()
                .expect("retained generation")
        ));
        assert!(!*original.cancelled.borrow());
    }

    settings(Some("repaired"), true, true);
    let repaired: control::ReloadResponse =
        control::request(&address, control::Command::Reload).await?;
    assert!(repaired.error.is_none());
    let status: control::Status = control::request(&address, control::Command::Status).await?;
    assert!(status.enabled);
    let _ = stop.send(());
    task.await.map_err(io::Error::other)??;
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
                    shared_generation_preserves_and_drains_tunnels_on_both_ports()
                        .await
                        .expect("shared Wizard token with aggregate proxy demand");
                    failed_reload_keeps_working_configuration()
                        .await
                        .expect("failed Reload preserves service and permits recovery");
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
