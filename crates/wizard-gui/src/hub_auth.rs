//! GUI wrapper around `wizard_core::hub::login`: the account tile state and the single browser
//! login attempt with an extendable deadline. Never include request URLs, headers, cookies or
//! HTTP-client errors in diagnostics; log only static names and error variants.

use crate::{
    hub_tokens::{self, HubTokensManager},
    revision_signal::{RevisionSignal, RevisionSnapshot},
    settings,
};
use alloc::sync::Arc;
use core::{future::Future, pin::pin, time::Duration};
use serde::Serialize;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tauri::Manager as _;
use tokio::time::Instant;
use wizard_core::hub::{
    self, HubError, HubProfile, Url,
    login::{self, CallbackServer, LoginError, Reply},
};

/// How long a login waits for the browser; a repeated login press restarts it.
const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);

/// The single login attempt. Login and logout decide what to do from it.
#[derive(Default)]
pub(crate) struct HubAuth(Mutex<Attempt>);

#[derive(Default)]
enum Attempt {
    #[default]
    Idle,
    /// Starting, or the callback arrived and the session is being exchanged and saved.
    Busy,
    /// Waiting for the browser; another login press reopens this page and extends the deadline.
    Waiting {
        authorization: Url,
        deadline: Deadline,
    },
}

impl HubAuth {
    fn attempt(&self) -> MutexGuard<'_, Attempt> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Login deadline that a repeated login press extends.
#[derive(Clone)]
struct Deadline(Arc<Mutex<Instant>>);

impl Deadline {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now() + LOGIN_TIMEOUT)))
    }

    fn get(&self) -> Instant {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn extend(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now() + LOGIN_TIMEOUT;
    }

    /// Runs `future`, failing with `Timeout` once the deadline passes without being extended.
    async fn run<T>(
        &self,
        future: impl Future<Output = Result<T, LoginError>>,
    ) -> Result<T, LoginError> {
        let mut future = pin!(future);
        loop {
            let at = self.get();
            if let Ok(result) = tokio::time::timeout_at(at, &mut future).await {
                return result;
            }
            if self.get() <= at {
                return Err(LoginError::Timeout);
            }
        }
    }
}

/// Profile of the signed-in account, kept in memory only: set at login, otherwise loaded from
/// Hub when first needed (`load_profile`), cleared on logout or a rejected session.
#[derive(Default)]
pub(crate) struct HubProfileState(Mutex<Option<HubProfile>>);

impl HubProfileState {
    fn lock(&self) -> MutexGuard<'_, Option<HubProfile>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct HubAccount {
    signed_in: bool,
    /// Absent until loaded from Hub, or when loading failed.
    profile: Option<HubProfile>,
    active_hub_id: Option<String>,
    pending: bool,
    error: Option<String>,
}

impl HubAccount {
    /// `signed_in` comes from saved settings (a read failure is signed out), `profile` from memory.
    fn saved(app: &tauri::AppHandle, pending: bool, error: Option<String>) -> Self {
        let settings = settings::get_state(app).ok();
        let signed_in = settings.as_ref().is_some_and(|s| !s.hub_session.is_empty());
        let profile = signed_in
            .then(|| app.state::<HubProfileState>().lock().clone())
            .flatten();
        let active_hub_id = profile
            .as_ref()
            .zip(settings.as_ref())
            .map(|(profile, settings)| profile.active_hub_id(&settings.active_hub_id).to_owned());
        Self {
            signed_in,
            profile,
            active_hub_id,
            pending,
            error,
        }
    }
}

/// Registers login state from saved settings.
pub(crate) fn initialize(app: &tauri::AppHandle) {
    app.manage(HubAuth::default());
    app.manage(HubProfileState::default());
    app.manage(RevisionSignal::new(HubAccount::saved(app, false, None)));
}

/// The signed-in account's profile, loading it from Hub when it is not in memory yet. A profile
/// loaded for a session that was replaced meanwhile is returned but not kept.
pub(crate) async fn load_profile(app: &tauri::AppHandle) -> Result<HubProfile, HubError> {
    if let Some(profile) = app.state::<HubProfileState>().lock().clone() {
        return Ok(profile);
    }

    let (sent, profile) = hub_tokens::with_session(app, |client, session| async move {
        let (update, profile) = hub::fetch_profile(&client, &session).await;
        let current_session = match &update {
            hub::SessionUpdate::Refreshed(refreshed) => refreshed.clone(),
            _ => session,
        };
        (update, profile.map(|profile| (current_session, profile)))
    })
    .await?;
    let current = settings::get_state(app).map(|s| s.hub_session);
    if current.is_ok_and(|current| current == sent) {
        *app.state::<HubProfileState>().lock() = Some(profile.clone());
        republish(app);
    }
    Ok(profile)
}

/// Forgets the profile on logout or a rejected session.
pub(crate) fn clear_profile(app: &tauri::AppHandle) {
    *app.state::<HubProfileState>().lock() = None;
}

/// Returns the account state once it differs from `last_revision`.
#[tauri::command]
pub(crate) async fn get_hub_account_state(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<HubAccount>>,
) -> Result<RevisionSnapshot<HubAccount>, String> {
    signal
        .wait(last_revision)
        .await
        .map_err(|_| "Не удалось получить состояние входа".into())
}

/// Starts a background login unless a session is saved. While the attempt waits for the
/// browser, reopens its page and extends its deadline instead.
#[tauri::command]
pub(crate) async fn login_hub_account(app: tauri::AppHandle) -> Result<(), String> {
    let reopen = {
        let auth = app.state::<HubAuth>();
        let mut attempt = auth.attempt();
        match &*attempt {
            Attempt::Busy => return Err("Вход уже выполняется".into()),
            Attempt::Waiting {
                authorization,
                deadline,
            } => {
                deadline.extend();
                authorization.clone()
            }
            Attempt::Idle => {
                let settings = settings::get_state(&app).map_err(|_| "Настройки недоступны")?;
                if !settings.hub_session.is_empty() {
                    return Ok(());
                }
                *attempt = Attempt::Busy;
                drop(attempt);
                start_login(app);
                return Ok(());
            }
        }
    };

    log::info!("hub login page reopened");
    login::open_browser(&reopen)
        .await
        .map_err(|error| error.message().to_owned())
}

/// Clears Wizard's saved session; browser sessions are untouched.
#[tauri::command]
pub(crate) async fn logout_hub_account(app: tauri::AppHandle) -> Result<(), String> {
    if !matches!(*app.state::<HubAuth>().attempt(), Attempt::Idle) {
        return Err("Дождитесь завершения попытки входа".into());
    }
    settings::update_state(&app, |s| {
        s.hub_session.clear();
        s.active_hub_id.clear();
    })
    .await
    .map_err(|_| "Не удалось удалить сессию Wizard")?;
    log::info!("hub logout");
    clear_profile(&app);
    HubTokensManager::clear(&app);
    publish(&app, false, None);
    Ok(())
}

/// Selects an accessible hub without changing any agent configuration.
#[tauri::command]
pub(crate) async fn select_hub(app: tauri::AppHandle, hub_id: String) -> Result<(), String> {
    let profile = load_profile(&app)
        .await
        .map_err(|_| "Не удалось загрузить хабы")?;
    let session = settings::get_state(&app)
        .map_err(|_| "Настройки недоступны")?
        .hub_session;
    if session.is_empty() {
        return Err("Войдите в CoreInfra Hub".into());
    }
    if !profile.hubs.iter().any(|hub| hub.hub_id == hub_id) {
        return Err("Хаб недоступен этому аккаунту".into());
    }
    settings::update_state(&app, move |s| {
        if s.hub_session == session {
            s.active_hub_id = hub_id;
        }
    })
    .await
    .map_err(|_| "Не удалось сохранить выбранный хаб")?;
    HubTokensManager::clear(&app);
    republish(&app);
    HubTokensManager::refresh(&app).await;
    Ok(())
}

/// Opens only the configured Hub origin in the system browser.
#[tauri::command]
pub(crate) async fn open_hub(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    let mut url = Url::parse(hub::HUB_ORIGIN).map_err(|_| "Некорректный адрес хаба")?;
    let account = HubAccount::saved(&app, false, None);
    if let Some(id) = account.active_hub_id {
        url.query_pairs_mut().append_pair("hub_id", &id);
    }
    if matches!(theme.as_str(), "light" | "dark" | "system") {
        url.query_pairs_mut().append_pair("app_theme", &theme);
    }
    login::open_browser(&url)
        .await
        .map_err(|error| error.message().to_owned())
}

/// Republishes the saved account after a session change, keeping `pending` and `error`.
pub(crate) fn republish(app: &tauri::AppHandle) {
    app.state::<RevisionSignal<HubAccount>>().update(|account| {
        Some(HubAccount::saved(
            app,
            account.pending,
            account.error.clone(),
        ))
    });
}

fn publish(app: &tauri::AppHandle, pending: bool, error: Option<LoginError>) {
    let error = error.map(|error| error.message().to_owned());
    app.state::<RevisionSignal<HubAccount>>()
        .notify(HubAccount::saved(app, pending, error));
}

/// Runs the attempt already marked `Busy` in the background.
fn start_login(app: tauri::AppHandle) {
    publish(&app, true, None);
    tauri::async_runtime::spawn(async move {
        let result = run_login(&app).await;
        *app.state::<HubAuth>().attempt() = Attempt::Idle;
        match result {
            Ok(()) => log::info!("hub login succeeded"),
            Err(error) => log::warn!("hub login failed: {error:?}"),
        }
        publish(&app, false, result.err());
        if result.is_ok() {
            HubTokensManager::refresh(&app).await;
        }
    });
}

/// Runs one login with `wizard_core::hub::login` steps. Waiting and network steps share one
/// deadline that a repeated login press extends. The settings write is never cancelled.
async fn run_login(app: &tauri::AppHandle) -> Result<(), LoginError> {
    let deadline = Deadline::new();
    let mut server = CallbackServer::bind().await?;
    let authorization = server.authorization_url();
    let client = hub::client().map_err(|_| LoginError::Client)?;

    let auth = app.state::<HubAuth>();
    *auth.attempt() = Attempt::Waiting {
        authorization: authorization.clone(),
        deadline: deadline.clone(),
    };
    let callback = async {
        deadline.run(login::open_browser(&authorization)).await?;
        deadline.run(server.next_link()).await
    }
    .await;
    *auth.attempt() = Attempt::Busy;
    let callback = callback?;
    drop(server);

    let (session, profile) = match deadline.run(callback.sign_in(&client)).await {
        Ok(signed_in) => signed_in,
        Err(error) => {
            callback.reply(Reply::Failed).await;
            return Err(error);
        }
    };
    if settings::update_state(app, move |s| {
        s.hub_session = session;
        s.active_hub_id.clear();
    })
    .await
    .is_err()
    {
        callback.reply(Reply::Failed).await;
        return Err(LoginError::Save);
    }
    *app.state::<HubProfileState>().lock() = Some(profile);
    callback.reply(Reply::SignedIn).await;
    Ok(())
}
