//! `account`: Hub account login, logout and status.

use anyhow::Result;
use clap::Subcommand;
use wizard_core::{
    hub::{
        self,
        login::{self as hub_login, CallbackServer, LoginError, Reply},
    },
    settings,
};

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
    /// Print the account as JSON (never includes the session).
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
            settings::update_file(|settings| {
                settings.hub_session.clear();
                settings.hub_profile = None;
            })?;
            eprintln!("Сессия Wizard удалена.");
        }
        AccountCommand::Status => {
            let settings = settings::load_from_file()?;
            let profile = settings.hub_profile.as_ref();
            let account = serde_json::json!({
                "signed_in": !settings.hub_session.is_empty(),
                "username": profile.map(|profile| &profile.username),
                "verified_email": profile.and_then(|profile| profile.verified_email.as_ref()),
            });
            println!("{}", serde_json::to_string_pretty(&account)?);
        }
    }
    Ok(())
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
    let account = match &profile.verified_email {
        Some(email) => format!("{} ({email})", profile.username),
        None => profile.username.clone(),
    };
    if settings::update_file(|settings| {
        settings.hub_session = session;
        settings.hub_profile = Some(profile);
    })
    .is_err()
    {
        callback.reply(Reply::Failed).await;
        return Err(LoginError::Save.into());
    }
    callback.reply(Reply::SignedIn).await;
    eprintln!("Вход выполнен: {account}");
    Ok(())
}
