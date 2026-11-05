//! GUI ownership of desktop application installation and UI-independent daily checks.

use crate::{
    agents::{AgentRevisionState, TokenChoice},
    hub_tokens,
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use alloc::{boxed::Box, sync::Arc};
use core::time::Duration;
use serde::Serialize;
use std::{
    sync::{Mutex, PoisonError},
    time::Instant,
};
use tauri::Manager as _;
use tokio::sync::oneshot;
use wizard_core::agents::{
    claude_desktop, codex,
    desktop_install::{self as installer, DesktopApp, SavedDownload},
};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Check,
    Download,
    Install,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum State {
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        path: Option<String>,
        notes: Vec<String>,
        confirmation: Option<String>,
        saved: Option<SavedDownload>,
    },
    /// `total` is 0 until known.
    Downloading {
        received: u64,
        total: u64,
        /// The connection failed; the download continues after a delay.
        reconnecting: bool,
    },
    ReadyToInstall {
        version: String,
        path: Option<String>,
        notes: Vec<String>,
        confirmation: Option<String>,
    },
    Installing {
        /// The user agreed to close the running applications, which open again afterwards.
        close: bool,
    },
    Installed {
        version: String,
        notices: Vec<String>,
    },
    Unsupported {
        message: String,
    },
    Failed {
        operation: Operation,
        message: String,
        /// A retried installation still needs this confirmation.
        confirmation: Option<String>,
    },
}

enum Package {
    Available(Box<installer::Release>),
    Downloaded(Arc<installer::DownloadedRelease>),
}

const CHECK_INTERVAL: Duration = Duration::from_hours(24);

#[derive(Default)]
struct Session {
    package: Option<Package>,
    last_check: Option<Instant>,
}

pub(crate) struct InstallManager {
    desktop: DesktopApp,
    signal: RevisionSignal<State>,
    session: Arc<tokio::sync::Mutex<Session>>,
    /// Pauses the running download.
    pause: Mutex<Option<oneshot::Sender<()>>>,
    unsupported: bool,
}

impl InstallManager {
    fn new(desktop: DesktopApp) -> Self {
        let reason = desktop.unsupported_reason();
        Self {
            desktop,
            unsupported: reason.is_some(),
            signal: RevisionSignal::new(
                reason.map_or(State::Idle, |message| State::Unsupported { message }),
            ),
            session: Arc::new(tokio::sync::Mutex::new(Session::default())),
            pause: Mutex::new(None),
        }
    }
}

/// One installer per application; operations of different applications run independently.
pub(crate) struct InstallManagers {
    claude: Arc<InstallManager>,
    chatgpt: Arc<InstallManager>,
}

impl Default for InstallManagers {
    fn default() -> Self {
        Self {
            claude: Arc::new(InstallManager::new(DesktopApp::Claude)),
            chatgpt: Arc::new(InstallManager::new(DesktopApp::ChatGpt)),
        }
    }
}

impl InstallManagers {
    fn get(&self, desktop: DesktopApp) -> &Arc<InstallManager> {
        match desktop {
            DesktopApp::Claude => &self.claude,
            DesktopApp::ChatGpt => &self.chatgpt,
        }
    }
}

/// UI state of an available release, possibly partly downloaded.
fn available_state(
    release: &installer::Release,
    notes: Vec<String>,
    saved: Option<SavedDownload>,
) -> State {
    State::Available {
        version: release.display_version.clone(),
        path: release.path(),
        notes,
        confirmation: release.confirmation(),
        saved,
    }
}

/// UI state of a downloaded release.
fn ready_state(release: &installer::Release, notes: Vec<String>) -> State {
    State::ReadyToInstall {
        version: release.display_version.clone(),
        path: release.path(),
        notes,
        confirmation: release.confirmation(),
    }
}

/// What earlier downloads saved toward `release`; downloads of other releases are deleted.
/// The cache only saves time, so its failures never fail an operation.
async fn saved_download(
    desktop: DesktopApp,
    release: Option<&installer::Release>,
) -> Option<SavedDownload> {
    let release = release.cloned();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        installer::saved_download(desktop, release.as_ref())
    })
    .await;
    if let Ok(Ok(saved)) = saved {
        saved
    } else {
        log::warn!("{} saved download could not be read", desktop.product());
        None
    }
}

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
            for desktop in DesktopApp::ALL {
                if app.state::<InstallManagers>().get(desktop).unsupported {
                    continue;
                }
                log::info!("Starting {} background check", desktop.product());
                if start(
                    app.clone(),
                    desktop,
                    None,
                    Operation::Check,
                    true,
                    false,
                    false,
                )
                .is_err()
                {
                    log::warn!("{} background check could not start", desktop.product());
                }
            }
        }
    });
}

#[tauri::command]
pub(crate) async fn get_desktop_install_state(
    desktop: DesktopApp,
    last_revision: Option<u32>,
    managers: tauri::State<'_, InstallManagers>,
) -> Result<RevisionSnapshot<State>, String> {
    managers
        .get(desktop)
        .signal
        .wait(last_revision)
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
pub(crate) fn check_desktop_release(
    app: tauri::AppHandle,
    desktop: DesktopApp,
    token: TokenChoice,
) -> Result<(), String> {
    start(
        app,
        desktop,
        Some(token),
        Operation::Check,
        false,
        false,
        false,
    )
}

#[tauri::command]
pub(crate) fn download_desktop_release(
    app: tauri::AppHandle,
    desktop: DesktopApp,
    token: TokenChoice,
) -> Result<(), String> {
    start(
        app,
        desktop,
        Some(token),
        Operation::Download,
        false,
        false,
        false,
    )
}

/// Stops the download. What it received stays saved, and downloading again continues it.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state by value"
)]
pub(crate) fn pause_desktop_download(
    desktop: DesktopApp,
    managers: tauri::State<'_, InstallManagers>,
) {
    let pause = managers
        .get(desktop)
        .pause
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    // Without a sender, or when sending fails, the download has already ended or been paused.
    if let Some(pause) = pause {
        pause.send(()).ok();
    }
}

/// Products that installing the downloaded release would close because they run now. Empty
/// without a downloaded release or while another operation runs.
#[tauri::command]
pub(crate) async fn get_desktop_running_apps(
    desktop: DesktopApp,
    managers: tauri::State<'_, InstallManagers>,
) -> Result<Vec<String>, String> {
    let target = {
        let Ok(session) = managers.get(desktop).session.try_lock() else {
            return Ok(Vec::new());
        };
        match &session.package {
            Some(Package::Downloaded(download)) => download.release.target.clone(),
            _ => return Ok(Vec::new()),
        }
    };
    tauri::async_runtime::spawn_blocking(move || installer::running_apps(&target))
        .await
        .map(|products| products.into_iter().map(str::to_owned).collect())
        .map_err(|_| format!("Не удалось проверить, запущен ли {}", desktop.product()))
}

/// `confirmed` approves the change that the selected release asks to confirm. `close` approves
/// closing the applications that the user was told run now.
#[tauri::command]
pub(crate) fn install_desktop_release(
    app: tauri::AppHandle,
    desktop: DesktopApp,
    confirmed: bool,
    close: bool,
) -> Result<(), String> {
    start(
        app,
        desktop,
        None,
        Operation::Install,
        false,
        confirmed,
        close,
    )
    .inspect_err(|message| {
        log::error!(
            "{} installation could not start: {message}",
            desktop.product()
        );
    })
}

fn start(
    app: tauri::AppHandle,
    desktop: DesktopApp,
    token: Option<TokenChoice>,
    operation: Operation,
    automatic: bool,
    confirmed: bool,
    close: bool,
) -> Result<(), String> {
    let manager = Arc::clone(app.state::<InstallManagers>().get(desktop));
    if manager.unsupported {
        return if automatic {
            Ok(())
        } else {
            Err(desktop.unsupported_reason().unwrap_or_default())
        };
    }
    let name = desktop.name();
    // Claim the operation before spawning; the owned guard keeps the mutex alive.
    let mut session = match Arc::clone(&manager.session).try_lock_owned() {
        Ok(session) => session,
        Err(_) if automatic => return Ok(()),
        Err(_) => return Err(format!("Операция с {name} уже выполняется")),
    };
    if automatic
        && (matches!(session.package, Some(Package::Downloaded(_)))
            || session
                .last_check
                .is_some_and(|last| last.elapsed() < CHECK_INTERVAL))
    {
        return Ok(());
    }

    let mut pause = None;
    match operation {
        Operation::Check if automatic => {} // Wait for an existing token before publishing Checking.
        Operation::Check => {
            // Checking again keeps a saved download only if the same release is found.
            session.package = None;
            // Failed checks also count, so a bad token or offline network cannot cause a retry loop.
            session.last_check = Some(Instant::now());
            manager.signal.notify(State::Checking);
        }
        Operation::Download => {
            if !matches!(session.package, Some(Package::Available(_))) {
                return Err("Сначала проверьте доступную версию".into());
            }
            let (sender, receiver) = oneshot::channel();
            *manager.pause.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
            pause = Some(receiver);
            manager.signal.notify(State::Downloading {
                received: 0,
                total: 0,
                reconnecting: false,
            });
        }
        Operation::Install => {
            if !matches!(session.package, Some(Package::Downloaded(_))) {
                return Err(format!("Сначала скачайте {name}"));
            }
            manager.signal.notify(State::Installing { close });
        }
    }

    tauri::async_runtime::spawn(async move {
        let result = run(
            &app,
            &manager,
            &mut session,
            token,
            operation,
            automatic,
            confirmed,
            close,
            pause,
        )
        .await;
        manager
            .pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if matches!(operation, Operation::Install) {
            // A failed replacement may still have changed the installation.
            app.state::<RevisionSignal<AgentRevisionState>>()
                .notify(AgentRevisionState);
        }
        if let Err(message) = result {
            if automatic {
                session.last_check = Some(Instant::now());
            }
            if matches!(operation, Operation::Install) {
                log::error!("{} installation failed: {message}", desktop.product());
            } else {
                // Check/download errors can originate in authenticated requests.
                log::warn!("{} installation operation failed", desktop.product());
            }
            // An installation that rejected the archive deleted it: offer to download it again.
            let operation = match (operation, &session.package) {
                (Operation::Install, Some(Package::Available(_))) => Operation::Download,
                _ => operation,
            };
            let confirmation = match &session.package {
                Some(Package::Downloaded(download)) if matches!(operation, Operation::Install) => {
                    download.release.confirmation()
                }
                _ => None,
            };
            manager.signal.notify(State::Failed {
                operation,
                message,
                confirmation,
            });
        }
    });
    Ok(())
}

async fn resolve_token(
    app: &tauri::AppHandle,
    desktop: DesktopApp,
    token: Option<TokenChoice>,
) -> Result<String, String> {
    match token {
        Some(TokenChoice::Existing { token }) if !token.is_empty() => Ok(token),
        Some(TokenChoice::Create) => {
            hub_tokens::issue(app, desktop.token_agent().token_name()).await
        }
        _ => Err("Выберите токен CoreInfra Hub".into()),
    }
}

/// The token in the application's routing configuration, else the Hub token Wizard created
/// for it.
async fn background_token(
    app: &tauri::AppHandle,
    desktop: DesktopApp,
) -> Result<Option<String>, String> {
    let settings = settings::get_state(app).map_err(|_| "Не удалось прочитать настройки Wizard")?;
    if settings.hub_session.is_empty() {
        return Ok(None);
    }
    let configured = tauri::async_runtime::spawn_blocking(move || match desktop {
        DesktopApp::Claude => claude_desktop::configured_token(),
        DesktopApp::ChatGpt => codex::configured_token(),
    })
    .await
    .map_err(|_| format!("Не удалось прочитать токен {}", desktop.product()))?;

    let hub_token = app
        .state::<RevisionSignal<hub_tokens::HubTokens>>()
        .current()
        .state
        .named_token(desktop.token_agent().token_name());
    if let Ok(token) = configured {
        Ok(token.or(hub_token))
    } else {
        log::warn!("{} configured token could not be read", desktop.product());
        Ok(hub_token)
    }
}

/// What else installation affects, for the user to read before installing.
async fn release_notes(
    app: &tauri::AppHandle,
    release: &installer::Release,
) -> Result<Vec<String>, String> {
    let mut notes = release.notes();
    if release.target.app() == DesktopApp::ChatGpt && release.updates_installation() {
        let settings = settings::get_state(app).map_err(|error| format!("{error:#}"))?;
        let bundled =
            tauri::async_runtime::spawn_blocking(move || codex::uses_bundled_daemon(&settings))
                .await
                .map_err(|_| "Не удалось найти Codex CLI")?;
        if bundled.unwrap_or(false) {
            notes.push(
                "Если фоновая служба Codex запущена, Wizard перезапустит её после установки: она использует Codex из ChatGPT. Это может прервать текущую работу Codex."
                    .to_owned(),
            );
        }
    }
    Ok(notes)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one operation's complete request"
)]
async fn run(
    app: &tauri::AppHandle,
    manager: &InstallManager,
    session: &mut Session,
    token: Option<TokenChoice>,
    operation: Operation,
    automatic: bool,
    confirmed: bool,
    close: bool,
    pause: Option<oneshot::Receiver<()>>,
) -> Result<(), String> {
    let desktop = manager.desktop;
    let name = desktop.name();
    match operation {
        Operation::Check => {
            let token = if automatic {
                let Some(token) = background_token(app, desktop).await? else {
                    return Ok(());
                };
                session.package = None;
                session.last_check = Some(Instant::now());
                manager.signal.notify(State::Checking);
                token
            } else {
                resolve_token(app, desktop, token).await?
            };
            let settings = settings::get_state(app).map_err(|error| format!("{error:#}"))?;
            let target = tauri::async_runtime::spawn_blocking(move || {
                let last_path = match desktop {
                    DesktopApp::Claude => settings.last_paths.claude_desktop,
                    DesktopApp::ChatGpt => settings.last_paths.codex_desktop,
                };
                installer::detect_target(desktop, last_path.as_deref())
            })
            .await
            .map_err(|_| format!("Не удалось проверить установленный {name}"))?
            .map_err(|error| format!("{error:#}"))?;

            let release = installer::check_release(&token, target)
                .await
                .map_err(|error| format!("{error:#}"))?;
            let Some(release) = release else {
                saved_download(desktop, None).await;
                session.package = None;
                manager.signal.notify(State::UpToDate);
                return Ok(());
            };
            let notes = release_notes(app, &release).await?;
            let mut saved = saved_download(desktop, Some(&release)).await;
            if saved.is_some_and(|saved| saved.complete()) {
                // A completely saved archive only needs verification, without the network.
                if let Ok(download) =
                    installer::download_release(release.clone(), &token, |_| {}).await
                {
                    log::info!("{} saved archive is ready to install", desktop.product());
                    session.package = Some(Package::Downloaded(Arc::new(download)));
                    manager.signal.notify(ready_state(&release, notes));
                    return Ok(());
                }
                log::warn!("{} saved archive could not be used", desktop.product());
                saved = saved_download(desktop, Some(&release)).await;
            }
            manager
                .signal
                .notify(available_state(&release, notes, saved));
            session.package = Some(Package::Available(Box::new(release)));
        }
        Operation::Download => {
            let Some(Package::Available(selected)) = session.package.as_ref() else {
                return Err("Сначала проверьте доступную версию".into());
            };
            let release = selected.as_ref().clone();
            let token = resolve_token(app, desktop, token).await?;
            let download = installer::download_release(release.clone(), &token, |progress| {
                manager.signal.notify(State::Downloading {
                    received: progress.received,
                    total: progress.total,
                    reconnecting: progress.reconnecting,
                });
            });
            // Without a pause request the sender is dropped, which disables that branch.
            let pause = pause.unwrap_or_else(|| oneshot::channel().1);
            let result = tokio::select! {
                result = download => Some(result),
                Ok(()) = pause => None,
            };
            let Some(result) = result else {
                log::info!("{} download paused", desktop.product());
                let saved = saved_download(desktop, Some(&release)).await;
                let notes = release_notes(app, &release).await?;
                manager
                    .signal
                    .notify(available_state(&release, notes, saved));
                return Ok(());
            };
            let download = match result {
                Ok(download) => download,
                Err(error) => {
                    let mut message = format!("{error:#}");
                    if saved_download(desktop, Some(&release)).await.is_some() {
                        message.push_str(
                            ". Скачанная часть сохранена, и скачивание продолжится с того же места",
                        );
                    }
                    return Err(message);
                }
            };
            let state = ready_state(
                &download.release,
                release_notes(app, &download.release).await?,
            );

            session.package = Some(Package::Downloaded(Arc::new(download)));
            manager.signal.notify(state);
        }
        Operation::Install => {
            let Some(Package::Downloaded(download)) = session.package.as_ref() else {
                return Err(format!("Сначала скачайте {name}"));
            };
            // Retain the cached archive even if the blocking task fails or panics.
            let download = Arc::clone(download);
            let version = download.release.display_version.clone();
            let result = tauri::async_runtime::spawn_blocking(move || {
                installer::install_release(&download, confirmed, close)
            })
            .await
            .map_err(|error| {
                let reason = match error {
                    tauri::Error::JoinError(error) if error.is_panic() => "panicked",
                    tauri::Error::JoinError(_) => "was cancelled",
                    _ => "failed",
                };
                // A panic payload is arbitrary data, not a safe diagnostic.
                log::error!("{} installation worker {reason}", desktop.product());
                format!("Не удалось выполнить установку {name}")
            })?;
            let mut installed = result.map_err(|error| {
                if installer::archive_rejected(&error)
                    && let Some(Package::Downloaded(download)) = &session.package
                {
                    // Installation deleted the archive: the release must be downloaded again.
                    session.package = Some(Package::Available(Box::new(download.release.clone())));
                }
                format!("{error:#}")
            })?;
            session.package = None;
            // Count the installation as the day's check, so its notices stay visible.
            session.last_check = Some(Instant::now());

            if desktop == DesktopApp::ChatGpt {
                // A daemon started from the replaced bundle keeps running the old executable.
                // Report failures as notices: the installation itself has succeeded.
                let app = app.clone();
                let restarted = tauri::async_runtime::spawn_blocking(move || {
                    codex::restart_bundled_daemon(&settings::get_state(&app)?)
                })
                .await;
                match restarted {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        log::error!("Codex daemon restart after ChatGPT installation failed");
                        installed.notices.push(format!(
                            "Не удалось перезапустить фоновую службу Codex: {error:#}"
                        ));
                    }
                    Err(_) => {
                        log::error!("Codex daemon restart worker failed");
                        installed.notices.push(
                            "Не удалось перезапустить фоновую службу Codex; выполните `codex app-server daemon restart` в терминале".to_owned(),
                        );
                    }
                }
            }
            manager.signal.notify(State::Installed {
                version,
                notices: installed.notices,
            });
        }
    }

    Ok(())
}
