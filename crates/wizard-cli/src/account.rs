//! `account`: Hub account login, logout and status.

use anyhow::{Result, anyhow};
use clap::Subcommand;
use wizard_core::hub::{
    self, HubError, SessionUpdate,
    login::{self as hub_login, CallbackServer, LoginError, Reply},
};
use wizard_settings as settings;

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// Sign in through a browser on this machine.
    Login {
        /// Print the Hub URL instead of opening a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget Wizard's Hub session; the browser session is untouched.
    Logout,
    /// Print the account as JSON, loading its profile from Hub (never includes the session).
    Status,
}

pub(crate) fn run(runtime: &tokio::runtime::Runtime, command: &AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Login { no_browser } => {
            if !settings::load_from_file()?.hub_session.is_empty() {
                eprintln!("Вход уже выполнен. Чтобы сменить аккаунт, выполните account logout.");
                return Ok(());
            }
            runtime.block_on(login(*no_browser))?;
        }
        AccountCommand::Logout => {
            settings::update_file(|settings| settings.hub_session.clear())?;
            eprintln!("Сессия Wizard удалена.");
        }
        AccountCommand::Status => {
            let (signed_in, profile) = runtime.block_on(status())?;
            let account = serde_json::json!({
                "signed_in": signed_in,
                "personal_hub_name": profile.as_ref().map(|profile| &profile.personal_hub_name),
            });
            println!("{}", serde_json::to_string_pretty(&account)?);
        }
    }
    Ok(())
}

/// Whether a session is saved and its profile from Hub. A rejected session is signed out; other
/// failures keep the session and report no profile.
async fn status() -> Result<(bool, Option<hub::HubProfile>)> {
    let session = settings::load_from_file()?.hub_session;
    if session.is_empty() {
        return Ok((false, None));
    }
    let client = hub::client().map_err(hub_error)?;
    let (update, profile) = hub::fetch_profile(&client, &session).await;
    apply_session(update, &session)?;
    match profile {
        Ok(profile) => Ok((true, Some(profile))),
        Err(HubError::SignedOut) => Ok((false, None)),
        Err(error) => {
            eprintln!("{:#}", hub_error(error));
            Ok((true, None))
        }
    }
}

/// Saves the session update and returns the session to send next.
pub(crate) fn apply_session(update: SessionUpdate, sent: &str) -> Result<String> {
    let next = match &update {
        SessionUpdate::Refreshed(session) => session.clone(),
        SessionUpdate::Unchanged | SessionUpdate::Rejected => sent.to_owned(),
    };
    if update != SessionUpdate::Unchanged {
        settings::update_file(|settings| update.apply(settings, sent))?;
    }
    Ok(next)
}

/// Hub errors carry no secrets; a rejected session was already signed out.
pub(crate) fn hub_error(error: HubError) -> anyhow::Error {
    match error {
        HubError::SignedOut => {
            anyhow!("Сессия CoreInfra Hub недействительна, выполните account login")
        }
        HubError::Request | HubError::Status(_) | HubError::Response => {
            anyhow!("Запрос к CoreInfra Hub не выполнен: {error:?}")
        }
    }
}

/// Signs in through the browser and saves the session. Ctrl-C cancels; nothing is saved then.
async fn login(no_browser: bool) -> Result<()> {
    let mut server = CallbackServer::bind().await?;
    let authorization = server.authorization_url();
    let client = hub::client().map_err(|_| LoginError::Client)?;

    if no_browser || hub_login::open_browser(&authorization).await.is_err() {
        eprintln!("Откройте в браузере на этом компьютере:\n{authorization}");
    }
    eprintln!("Подтвердите вход в браузере. Ctrl-C отменяет вход.");
    let callback = server.next_link().await?;
    drop(server);

    let (session, profile) = match callback.sign_in(&client).await {
        Ok(signed_in) => signed_in,
        Err(error) => {
            callback.reply(Reply::Failed).await;
            return Err(error.into());
        }
    };
    if settings::update_file(|settings| settings.hub_session = session).is_err() {
        callback.reply(Reply::Failed).await;
        return Err(LoginError::Save.into());
    }
    callback.reply(Reply::SignedIn).await;
    eprintln!("Вход выполнен: {}", profile.personal_hub_name);
    Ok(())
}
