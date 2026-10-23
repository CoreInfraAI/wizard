//! GUI ownership of Claude installation and UI-independent daily checks.

#[cfg(not(target_os = "macos"))]
use crate::agents::TokenChoice;
use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
#[cfg(target_os = "macos")]
use crate::{
    agents::{AgentRevisionState, TokenChoice},
    hub_tokens, settings,
};
#[cfg(target_os = "macos")]
use alloc::boxed::Box;
use alloc::sync::Arc;
#[cfg(target_os = "macos")]
use core::time::Duration;
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::time::Instant;
#[cfg(target_os = "macos")]
use tauri::Manager as _;
#[cfg(target_os = "macos")]
use wizard_core::agents::{TokenAgent, claude_desktop_install as installer};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Check,
    Download,
    Install,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "macOS-only operation phases")
)]
pub(crate) enum State {
    #[cfg(not(target_os = "macos"))]
    Unsupported,
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        path: String,
    },
    Downloading {
        received: u64,
        total: u64,
    },
    ReadyToInstall {
        version: String,
        path: String,
    },
    Installing,
    Failed {
        operation: Operation,
        message: String,
    },
}

#[cfg(target_os = "macos")]
enum Package {
    Available(Box<installer::Release>),
    Downloaded(Arc<installer::DownloadedRelease>),
}

#[cfg(target_os = "macos")]
const CHECK_INTERVAL: Duration = Duration::from_hours(24);

#[cfg(target_os = "macos")]
#[derive(Default)]
struct Session {
    package: Option<Package>,
    last_check: Option<Instant>,
}

pub(crate) struct InstallManager {
    signal: RevisionSignal<State>,
    #[cfg(target_os = "macos")]
    session: Arc<tokio::sync::Mutex<Session>>,
}

impl Default for InstallManager {
    fn default() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            signal: RevisionSignal::new(State::Idle),
            #[cfg(not(target_os = "macos"))]
            signal: RevisionSignal::new(State::Unsupported),
            #[cfg(target_os = "macos")]
            session: Arc::new(tokio::sync::Mutex::new(Session::default())),
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn start_background_checks(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let tokens = app.state::<RevisionSignal<hub_tokens::HubTokens>>();
        let mut revision = None;
        // Only inspect the deadline here; network checks are limited by last_check.
        // This also retries a due check skipped while another operation held the lock.
        let mut timer = tokio::time::interval(Duration::from_mins(5));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = timer.tick() => {}
                snapshot = tokens.wait(revision) => {
                    let Ok(snapshot) = snapshot else { break };
                    revision = Some(snapshot.revision);
                }
            }

            if !settings::get_state(&app).is_ok_and(|settings| !settings.hub_session.is_empty()) {
                continue;
            }

            log::info!("Starting Claude Desktop background check");
            if start(app.clone(), None, Operation::Check, true).is_err() {
                log::warn!("Claude Desktop background check could not start");
            }
        }
    });
}

#[tauri::command]
pub(crate) async fn get_claude_desktop_install_state(
    last_revision: Option<u32>,
    manager: tauri::State<'_, Arc<InstallManager>>,
) -> Result<RevisionSnapshot<State>, String> {
    manager
        .signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub(crate) fn check_claude_desktop_release(
    app: tauri::AppHandle,
    token: TokenChoice,
) -> Result<(), String> {
    start(app, Some(token), Operation::Check, false)
}

#[tauri::command]
pub(crate) fn download_claude_desktop_release(
    app: tauri::AppHandle,
    token: TokenChoice,
) -> Result<(), String> {
    start(app, Some(token), Operation::Download, false)
}

#[tauri::command]
pub(crate) fn install_claude_desktop_release(app: tauri::AppHandle) -> Result<(), String> {
    start(app, None, Operation::Install, false)
}

#[cfg(not(target_os = "macos"))]
fn start(
    _app: tauri::AppHandle,
    _token: Option<TokenChoice>,
    _operation: Operation,
    _automatic: bool,
) -> Result<(), String> {
    Err("Установка Claude доступна только на macOS".into())
}

#[cfg(target_os = "macos")]
fn start(
    app: tauri::AppHandle,
    token: Option<TokenChoice>,
    operation: Operation,
    automatic: bool,
) -> Result<(), String> {
    let manager = Arc::clone(&app.state::<Arc<InstallManager>>());
    // Claim the operation before spawning; the owned guard keeps the mutex alive.
    let mut session = match Arc::clone(&manager.session).try_lock_owned() {
        Ok(session) => session,
        Err(_) if automatic => return Ok(()),
        Err(_) => return Err("Операция с Claude уже выполняется".into()),
    };
    if automatic
        && (matches!(session.package, Some(Package::Downloaded(_)))
            || session
                .last_check
                .is_some_and(|last| last.elapsed() < CHECK_INTERVAL))
    {
        return Ok(());
    }

    match operation {
        Operation::Check if automatic => {} // Wait for an existing token before publishing Checking.
        Operation::Check => {
            // Explicitly checking again discards the previous selection and any cached archive.
            session.package = None;
            // Failed checks also count, so a bad token or offline network cannot cause a retry loop.
            session.last_check = Some(Instant::now());
            manager.signal.notify(State::Checking);
        }
        Operation::Download => {
            if !matches!(session.package, Some(Package::Available(_))) {
                return Err("Сначала проверьте доступную версию".into());
            }

            manager.signal.notify(State::Downloading {
                received: 0,
                total: 0,
            });
        }
        Operation::Install => {
            if !matches!(session.package, Some(Package::Downloaded(_))) {
                return Err("Сначала скачайте Claude".into());
            }

            manager.signal.notify(State::Installing);
        }
    }

    tauri::async_runtime::spawn(async move {
        let result = run(&app, &manager, &mut session, token, operation, automatic).await;

        if matches!(operation, Operation::Install) {
            // A failed replacement may still have changed the installation.
            app.state::<RevisionSignal<AgentRevisionState>>()
                .notify(AgentRevisionState);
        }

        if let Err(message) = result {
            if automatic {
                session.last_check = Some(Instant::now());
            }
            log::warn!("Claude Desktop installation operation failed");
            manager.signal.notify(State::Failed { operation, message });
        }
    });

    Ok(())
}

#[cfg(target_os = "macos")]
async fn resolve_token(
    app: &tauri::AppHandle,
    token: Option<TokenChoice>,
) -> Result<String, String> {
    match token {
        Some(TokenChoice::Existing { token }) if !token.is_empty() => Ok(token),
        Some(TokenChoice::Create) => {
            hub_tokens::issue(app, TokenAgent::ClaudeDesktop.token_name()).await
        }
        _ => Err("Выберите токен CoreInfra Hub".into()),
    }
}

#[cfg(target_os = "macos")]
async fn background_token(app: &tauri::AppHandle) -> Result<Option<String>, String> {
    let settings = settings::get_state(app).map_err(|_| "Не удалось прочитать настройки Wizard")?;
    if settings.hub_session.is_empty() {
        return Ok(None);
    }
    let configured = tauri::async_runtime::spawn_blocking(move || {
        wizard_core::agents::claude_desktop::configured_token(&settings)
    })
    .await
    .map_err(|_| "Не удалось прочитать токен Claude Desktop")?;

    let hub_token = app
        .state::<RevisionSignal<hub_tokens::HubTokens>>()
        .current()
        .state
        .named_token(TokenAgent::ClaudeDesktop.token_name());
    if let Ok(token) = configured {
        Ok(token.or(hub_token))
    } else {
        log::warn!("Claude Desktop configured token could not be read");
        Ok(hub_token)
    }
}

#[cfg(target_os = "macos")]
async fn run(
    app: &tauri::AppHandle,
    manager: &InstallManager,
    session: &mut Session,
    token: Option<TokenChoice>,
    operation: Operation,
    automatic: bool,
) -> Result<(), String> {
    match operation {
        Operation::Check => {
            let token = if automatic {
                let Some(token) = background_token(app).await? else {
                    return Ok(());
                };
                session.package = None;
                session.last_check = Some(Instant::now());
                manager.signal.notify(State::Checking);
                token
            } else {
                resolve_token(app, token).await?
            };
            let settings = settings::get_state(app).map_err(|error| format!("{error:#}"))?;
            let target = tauri::async_runtime::spawn_blocking(move || {
                installer::detect_target(settings.last_paths.claude_desktop.as_deref())
            })
            .await
            .map_err(|_| "Не удалось проверить установленный Claude")?
            .map_err(|error| format!("{error:#}"))?;

            let release = installer::check_release(&token, target)
                .await
                .map_err(|error| format!("{error:#}"))?;
            let state = match &release {
                Some(value) => State::Available {
                    version: value.version.to_string(),
                    path: value.target.path.display().to_string(),
                },
                None => State::UpToDate,
            };

            session.package = release.map(|release| Package::Available(Box::new(release)));
            manager.signal.notify(state);
        }
        Operation::Download => {
            let Some(Package::Available(selected)) = session.package.as_ref() else {
                return Err("Сначала проверьте доступную версию".into());
            };
            let token = resolve_token(app, token).await?;
            let download = installer::download_release(
                selected.as_ref().clone(),
                &token,
                |received, total| {
                    manager
                        .signal
                        .notify(State::Downloading { received, total });
                },
            )
            .await
            .map_err(|error| format!("{error:#}"))?;
            let state = State::ReadyToInstall {
                version: download.release.version.to_string(),
                path: download.release.target.path.display().to_string(),
            };

            session.package = Some(Package::Downloaded(Arc::new(download)));
            manager.signal.notify(state);
        }
        Operation::Install => {
            let Some(Package::Downloaded(download)) = session.package.as_ref() else {
                return Err("Сначала скачайте Claude".into());
            };
            // Retain the cached archive even if the blocking task fails or panics.
            let download = Arc::clone(download);
            tauri::async_runtime::spawn_blocking(move || installer::install_release(&download))
                .await
                .map_err(|_| "Не удалось выполнить установку Claude")?
                .map_err(|error| format!("{error:#}"))?;

            session.package = None;
            manager.signal.notify(State::Idle);
        }
    }

    Ok(())
}
