//! Open a known agent in its desktop app or an interactive terminal.

use super::{AgentDetection, desktop_install, detection};
use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use std::path::Path;
use wizard_settings::Settings;

/// The GUI chooses an agent, never an executable or shell command.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchAgent {
    ClaudeDesktop,
    #[serde(rename = "chatgpt")]
    ChatGpt,
    Claude,
    Codex,
    #[serde(rename = "opencode")]
    OpenCode,
    Pi,
}

/// Must be called in a Tokio blocking worker, as discovery can run a version check.
pub fn launch(agent: LaunchAgent, settings: &Settings) -> Result<()> {
    let paths = &settings.last_paths;
    let (cli, last_path) = match agent {
        LaunchAgent::ClaudeDesktop | LaunchAgent::ChatGpt => {
            let (desktop, last_path) = match agent {
                LaunchAgent::ClaudeDesktop => (
                    desktop_install::DesktopApp::Claude,
                    paths.claude_desktop.as_deref(),
                ),
                _ => (
                    desktop_install::DesktopApp::ChatGpt,
                    paths.codex_desktop.as_deref(),
                ),
            };
            return desktop_install::open_existing(desktop, last_path);
        }
        LaunchAgent::Claude => (detection::Agent::Claude, paths.claude.as_deref()),
        LaunchAgent::Codex => (detection::Agent::Codex, paths.codex.as_deref()),
        LaunchAgent::OpenCode => (detection::Agent::OpenCode, paths.opencode.as_deref()),
        LaunchAgent::Pi => (detection::Agent::Pi, paths.pi.as_deref()),
    };
    match detection::detect(cli, last_path) {
        AgentDetection::Found(info) => open_terminal(&info.path),
        AgentDetection::NotFound => bail!("Агент больше не найден. Вернитесь к выбору агента."),
        AgentDetection::Error(error) => bail!("Не удалось найти агента: {error}"),
    }
}

#[cfg(target_os = "macos")]
fn terminal_line(executable: &Path, directory: &Path) -> Result<String> {
    fn quote(path: &Path) -> Result<String> {
        let value = path
            .to_str()
            .context("Путь агента содержит неподдерживаемые символы")?;
        Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
    }
    Ok(format!(
        "cd -- {} && exec {}",
        quote(directory)?,
        quote(executable)?
    ))
}

#[cfg(target_os = "macos")]
fn terminal_script(executable: &Path, directory: &Path) -> Result<String> {
    // Terminal opens the script through Launch Services, without an Apple Events permission.
    // Remove the temporary launcher as soon as Terminal reads it; it contains no credentials.
    Ok(format!(
        "#!/bin/sh\nrm -f -- \"$0\"\n{}\n",
        terminal_line(executable, directory)?
    ))
}

#[cfg(target_os = "macos")]
fn open_terminal(executable: &Path) -> Result<()> {
    use core::time::Duration;
    use std::{fs, io::Write as _, os::unix::fs::PermissionsExt as _, process::Stdio};

    let home = dirs::home_dir().context("Не удалось найти домашнюю папку")?;
    let mut launcher = tempfile::Builder::new()
        .prefix("wizard-agent-")
        .suffix(".command")
        .tempfile()
        .context("Не удалось подготовить запуск агента")?;
    launcher.write_all(terminal_script(executable, &home)?.as_bytes())?;
    launcher
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o700))?;
    let (_, path) = launcher
        .keep()
        .context("Не удалось сохранить команду запуска")?;
    let mut command = tokio::process::Command::new("/usr/bin/open");
    command
        .args(["-b", "com.apple.Terminal"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let result = crate::platform::command_output(command, Duration::from_secs(30))
        .context("Не удалось открыть Терминал")
        .and_then(|output| {
            anyhow::ensure!(
                output.status.success(),
                "Не удалось открыть Терминал. Повторите попытку."
            );
            Ok(())
        });
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(target_os = "windows")]
fn open_terminal(executable: &Path) -> Result<()> {
    use std::{os::windows::process::CommandExt as _, process::Command};
    use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;

    // A literal environment value avoids interpreting quotes/metacharacters in a path.
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NoExit",
            "-Command",
            "& $env:WIZARD_LAUNCH_AGENT",
        ])
        .env("WIZARD_LAUNCH_AGENT", executable)
        .creation_flags(CREATE_NEW_CONSOLE);
    if let Some(home) = dirs::home_dir() {
        command.current_dir(home);
    }
    command.spawn().context("Не удалось открыть PowerShell")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_terminal(executable: &Path) -> Result<()> {
    use std::{io::ErrorKind, process::Command};

    for (terminal, separator) in [
        ("x-terminal-emulator", "-e"),
        ("gnome-terminal", "--"),
        ("konsole", "-e"),
        ("xfce4-terminal", "-x"),
        ("xterm", "-e"),
    ] {
        let mut command = Command::new(terminal);
        command.arg(separator).arg(executable);
        if let Some(home) = dirs::home_dir() {
            command.current_dir(home);
        }
        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Не удалось открыть терминал"),
        }
    }
    bail!("Терминал не найден. Установите терминал или запустите агента вручную.")
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn open_terminal(_: &Path) -> Result<()> {
    bail!("Запуск агента на этой платформе не поддерживается")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_accepts_only_known_agent_ids() {
        for id in [
            "claude_desktop",
            "chatgpt",
            "claude",
            "codex",
            "opencode",
            "pi",
        ] {
            assert!(serde_json::from_value::<LaunchAgent>(serde_json::json!(id)).is_ok());
        }
        for id in [
            "/bin/sh",
            "claude --help",
            "$(touch /tmp/wizard-launch)",
            "open_code",
        ] {
            assert!(serde_json::from_value::<LaunchAgent>(serde_json::json!(id)).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_paths_are_literal_shell_data() -> Result<()> {
        use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};
        let directory = tempfile::tempdir()?;
        let workspace = directory.path().join("work ' \" $HOME; $(touch injected)");
        fs::create_dir(&workspace)?;
        let executable = directory.path().join("agent ' \" $HOME; $(touch injected)");
        fs::write(&executable, "#!/bin/sh\nprintf 'launched\\n'\npwd\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let launcher = directory.path().join("wizard.command");
        fs::write(&launcher, terminal_script(&executable, &workspace)?)?;
        let output = Command::new("/bin/sh")
            .arg(&launcher)
            .current_dir(directory.path())
            .output()?;
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout)?;
        assert_eq!(text, format!("launched\n{}\n", workspace.display()));
        assert!(!launcher.exists(), "the temporary command removes itself");
        assert!(!directory.path().join("injected").exists());
        assert!(!workspace.join("injected").exists());
        Ok(())
    }
}
