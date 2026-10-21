//! `agent`: agent state and routing plans with a `CoreInfra` Hub token.

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::{Args, Subcommand};
use std::io::{self, IsTerminal as _, Write as _};
use wizard_core::{
    agents::{self, HubMode, RoutingMode, TokenAgent, claude, codex, opencode, pi},
    hub::{self, HubError, SessionUpdate},
    prepared_plan::PreparedPlan,
    settings, validate_token,
};

#[derive(Subcommand)]
pub(crate) enum AgentCommand {
    /// Print agent state as JSON (token values are redacted).
    Status,
    /// Manage the Codex routing configuration.
    #[command(disable_help_subcommand = true)]
    Codex {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Manage the Claude Code routing configuration.
    #[command(disable_help_subcommand = true)]
    Claude {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Claude Desktop configuration is available only in the GUI.
    #[command(disable_help_subcommand = true)]
    ClaudeDesktop {
        #[command(subcommand)]
        command: RoutingCommand,
    },
    /// Manage the Pi Hub plugin.
    #[command(disable_help_subcommand = true)]
    Pi {
        #[command(subcommand)]
        command: HubCommand,
    },
    /// Manage the `OpenCode` Hub plugin.
    #[command(disable_help_subcommand = true)]
    Opencode {
        #[command(subcommand)]
        command: HubCommand,
    },
}

/// Without `--token-stdin`: the agent's configured token, else the oldest `CoreInfra` Hub token
/// named after the agent, else a new one (requires `account login`).
#[derive(Args)]
pub(crate) struct TokenArgs {
    /// Read the token from stdin instead of the agent configuration or Hub; whitespace is trimmed.
    #[arg(long)]
    token_stdin: bool,
}

#[derive(Subcommand)]
pub(crate) enum RoutingCommand {
    /// Use `CoreInfra` Hub.
    Hub(TokenArgs),
    /// Use `CoreInfra` Sub with existing subscription authentication.
    Sub(TokenArgs),
    /// Remove the `CoreInfra` routing configuration.
    None,
}

#[derive(Subcommand)]
pub(crate) enum HubCommand {
    /// Configure Hub, installing a plugin when required.
    Hub(TokenArgs),
    /// Remove the `CoreInfra` Hub configuration and its token.
    None,
}

pub(crate) fn run(
    runtime: &tokio::runtime::Runtime,
    yes: bool,
    command: AgentCommand,
) -> Result<()> {
    match command {
        AgentCommand::Status => {
            let settings = settings::load_from_file()?;
            let mut state = runtime.block_on(agents::collect_agent_state(&settings));
            settings::update_file(|settings| state.update_settings_paths(settings))?;
            state.redact_tokens();
            println!("{}", serde_json::to_string_pretty(&state)?);
        }
        AgentCommand::Codex { command } => {
            let (mode, args) = routing_mode(&command);
            let Some(token) = resolve_token(
                runtime,
                yes,
                args,
                TokenAgent::Codex,
                codex::configured_token,
            )?
            else {
                return Ok(());
            };
            let settings = settings::load_from_file()?;

            run_plan(runtime, yes, move || {
                codex::prepare_routing(mode, &token, &settings)
            })?;
        }
        AgentCommand::Claude { command } => {
            let (mode, args) = routing_mode(&command);
            let Some(token) = resolve_token(
                runtime,
                yes,
                args,
                TokenAgent::Claude,
                claude::configured_token,
            )?
            else {
                return Ok(());
            };

            run_plan(runtime, yes, move || claude::prepare_routing(mode, &token))?;
        }
        AgentCommand::ClaudeDesktop { .. } => {
            anyhow::bail!(
                "Настройка Claude Desktop доступна только в GUI: прокси-сервер запускается только в GUI."
            );
        }
        AgentCommand::Pi { command } => {
            let (mode, args) = hub_mode(&command);
            let Some(token) =
                resolve_token(runtime, yes, args, TokenAgent::Pi, pi::configured_token)?
            else {
                return Ok(());
            };
            let settings = settings::load_from_file()?;

            run_plan(runtime, yes, move || {
                pi::prepare_hub(mode, &token, &settings)
            })?;
        }
        AgentCommand::Opencode { command } => {
            let (mode, args) = hub_mode(&command);
            let Some(token) = resolve_token(
                runtime,
                yes,
                args,
                TokenAgent::OpenCode,
                opencode::configured_token,
            )?
            else {
                return Ok(());
            };
            let settings = settings::load_from_file()?;

            run_plan(runtime, yes, move || {
                opencode::prepare_hub(mode, &token, &settings)
            })?;
        }
    }
    Ok(())
}

/// The mode and, for enabled modes, how to obtain the token.
const fn routing_mode(command: &RoutingCommand) -> (RoutingMode, Option<&TokenArgs>) {
    match command {
        RoutingCommand::Hub(args) => (RoutingMode::Hub, Some(args)),
        RoutingCommand::Sub(args) => (RoutingMode::Sub, Some(args)),
        RoutingCommand::None => (RoutingMode::Disabled, None),
    }
}

const fn hub_mode(command: &HubCommand) -> (HubMode, Option<&TokenArgs>) {
    match command {
        HubCommand::Hub(args) => (HubMode::Hub, Some(args)),
        HubCommand::None => (HubMode::Disabled, None),
    }
}

/// Token for the plan: empty when disabled; from stdin with `--token-stdin`; else the agent's
/// configured token; else the oldest Hub token named after `agent`; else a new Hub token after
/// confirmation. `None` means the user declined creating one. Never print the value.
fn resolve_token(
    runtime: &tokio::runtime::Runtime,
    yes: bool,
    args: Option<&TokenArgs>,
    agent: TokenAgent,
    configured: fn() -> Result<Option<String>>,
) -> Result<Option<String>> {
    let Some(args) = args else {
        return Ok(Some(String::new()));
    };

    if args.token_stdin {
        let mut token = String::new();
        io::stdin().read_line(&mut token)?;
        let token = token.trim().to_owned();
        ensure!(!token.is_empty(), "token is empty");
        validate_token(&token)?;
        return Ok(Some(token));
    }

    if let Some(token) = configured()? {
        eprintln!("Используется токен из конфигурации агента.");
        return Ok(Some(token));
    }
    runtime.block_on(hub_token(yes, agent.token_name()))
}

/// The oldest Hub token named `name`, or a new one after confirmation. Saves refreshed sessions
/// and signs out on a rejected one, as the GUI does.
async fn hub_token(yes: bool, name: &str) -> Result<Option<String>> {
    let settings = settings::load_from_file()?;
    ensure!(
        !settings.hub_session.is_empty(),
        "Войдите в CoreInfra Hub (account login) или передайте токен через --token-stdin"
    );
    let hub_id = settings
        .hub_profile
        .map(|profile| profile.personal_hub_id)
        .filter(|id| !id.is_empty())
        .context("Войдите в аккаунт CoreInfra Hub заново: account logout, затем account login")?;
    let client = hub::client().map_err(hub_error)?;

    let session = settings.hub_session;
    let (update, tokens) = hub::list_agent_tokens(&client, &session, &hub_id).await;
    let session = apply_session(update, &session)?;
    if let Some(token) = tokens.map_err(hub_error)?.named(name) {
        eprintln!("Используется токен {name} из CoreInfra Hub.");
        return Ok(Some(token.token.clone()));
    }

    if !confirm(yes, &format!("Создать токен {name} в CoreInfra Hub?"))? {
        eprintln!("Отменено. Изменения не применены.");
        return Ok(None);
    }
    let (update, token) = hub::issue_agent_token(&client, &session, &hub_id, name).await;
    apply_session(update, &session)?;
    let token = token.map_err(hub_error)?;
    eprintln!("Токен {name} создан в CoreInfra Hub.");
    Ok(Some(token))
}

/// Saves the session update and returns the session to send next.
fn apply_session(update: SessionUpdate, sent: &str) -> Result<String> {
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
fn hub_error(error: HubError) -> anyhow::Error {
    match error {
        HubError::SignedOut => {
            anyhow!("Сессия CoreInfra Hub недействительна, выполните account login")
        }
        HubError::Request | HubError::Status(_) | HubError::Response => {
            anyhow!("Запрос к CoreInfra Hub не выполнен: {error:?}")
        }
    }
}

/// Asks on the terminal unless `yes`; non-interactive execution requires `--yes`.
fn confirm(yes: bool, question: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        bail!("non-interactive execution requires --yes; nothing was changed");
    }

    eprint!("{question} [y/N] ");
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn run_plan(
    runtime: &tokio::runtime::Runtime,
    yes: bool,
    prepare: impl FnOnce() -> Result<PreparedPlan> + Send + 'static,
) -> Result<()> {
    let plan = runtime.block_on(async {
        tokio::task::spawn_blocking(prepare)
            .await
            .context("plan preparation task failed")?
    })?;

    let preview = plan.info();
    if preview.is_empty() {
        eprintln!("Изменения не требуются.");
        return Ok(());
    }

    eprintln!("Wizard выполнит:");
    for (index, info) in preview.iter().enumerate() {
        eprintln!("{}. {}", index + 1, info.description);
    }
    eprintln!("Завершите текущую работу и закройте агента перед применением.");

    if !confirm(yes, "Применить?")? {
        eprintln!("Отменено. Изменения не применены.");
        return Ok(());
    }

    runtime.block_on(async {
        tokio::task::spawn_blocking(move || plan.apply())
            .await
            .context("plan application task failed")?
    })?;

    eprintln!("Готово.");
    Ok(())
}
