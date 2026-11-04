//! Turns off the `ChatGPT` app's own Sparkle updater while Wizard owns its updates.
//! `ChatGPT` reads `CODEX_SPARKLE_ENABLED` at launch. Applications started from Finder or the
//! Dock inherit `launchctl setenv` values, and a login agent sets the value again after login.

use crate::prepared_plan::PreparedStep;
use anyhow::{Context as _, Result, ensure};
use std::{
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::PathBuf,
    process::{Command, Stdio},
};

const LABEL: &str = "ai.coreinfra.wizard.chatgpt-updates";
const VARIABLE: &str = "CODEX_SPARKLE_ENABLED";
const AGENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ai.coreinfra.wizard.chatgpt-updates</string>
    <key>AssociatedBundleIdentifiers</key>
    <array>
        <string>ai.coreinfra.wizard</string>
    </array>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/launchctl</string>
        <string>setenv</string>
        <string>CODEX_SPARKLE_ENABLED</string>
        <string>false</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#;

fn agent_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("failed to resolve home directory")?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// Whether leaving `ChatGPT`'s updater `disabled` has anything to set or remove.
fn needed(disabled: bool) -> Result<bool> {
    Ok(disabled
        || agent_path()?
            .try_exists()
            .context("failed to check the ChatGPT updates login item")?)
}

/// The step that leaves `ChatGPT`'s updater `disabled`, if anything is to change or restore.
pub(super) fn step(disabled: bool) -> Result<Option<PreparedStep>> {
    Ok(needed(disabled)?.then_some(PreparedStep::SetChatGptUpdatesDisabled { disabled }))
}

/// Applies the same change directly, such as after restored files changed the routing mode.
pub(super) fn reconcile(disabled: bool) -> Result<()> {
    if needed(disabled)? {
        set_disabled(disabled)?;
    }
    Ok(())
}

/// Takes effect when `ChatGPT` starts next. Call from a blocking worker.
pub(crate) fn set_disabled(disabled: bool) -> Result<()> {
    let path = agent_path()?;
    if disabled {
        let directory = path.parent().context("login item path has no parent")?;
        std::fs::create_dir_all(directory)
            .with_context(|| format!("failed to create {}", directory.display()))?;
        // Replace whatever is at the path, including a symlink, instead of writing through it.
        let mut file = tempfile::NamedTempFile::new_in(directory)
            .context("failed to create the ChatGPT updates login item")?;
        file.write_all(AGENT.as_bytes())
            .and_then(|()| {
                file.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o644))
            })
            .context("failed to write the ChatGPT updates login item")?;
        file.persist(&path)
            .with_context(|| format!("failed to save {}", path.display()))?;
        launchctl(&["setenv", VARIABLE, "false"])
    } else {
        launchctl(&["unsetenv", VARIABLE])?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("failed to remove {}", path.display()))
            }
        }
    }
}

fn launchctl(args: &[&str]) -> Result<()> {
    let status = Command::new("/bin/launchctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run launchctl")?;
    ensure!(
        status.success(),
        "launchctl {} failed with {status}",
        args.join(" ")
    );
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "bounded test fixture")]
mod tests {
    use super::*;

    #[test]
    fn login_item_sets_the_variable_that_chatgpt_reads() {
        let agent: plist::Dictionary = plist::from_bytes(AGENT.as_bytes()).unwrap();
        assert_eq!(agent["Label"].as_string(), Some(LABEL));
        let arguments: Vec<_> = agent["ProgramArguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|argument| argument.as_string().unwrap())
            .collect();
        assert_eq!(arguments, ["/bin/launchctl", "setenv", VARIABLE, "false"]);
        assert_eq!(agent["RunAtLoad"].as_boolean(), Some(true));
    }
}
