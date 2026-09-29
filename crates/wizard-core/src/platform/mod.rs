use core::time::Duration;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::{Context as _, Result};

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// Reads a nonempty environment variable, logging only its name when unavailable.
pub(crate) fn env_var_not_empty(name: &str) -> Option<OsString> {
    let value = std::env::var_os(name).filter(|value| !value.is_empty());
    if value.is_none() {
        log::info!("environment variable {name} is unset or empty");
    }
    value
}

/// Searches common directories, extra directories, the per-user Nix profile, then PATH.
/// Expands a leading `~` path component. On Windows, also expands leading `%VAR%/`
/// and checks `.exe` before `.cmd` in each directory.
pub(crate) fn find_executable(name: &str, extra_paths: Vec<PathBuf>) -> Result<Option<PathBuf>> {
    // Common to all supported platforms.
    let mut paths = vec!["~/.local/bin", "~/.bun/bin"];

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    paths.extend([
        "/usr/local/bin",
        "~/.local/lib/bin",
        "~/.npm-global/bin",
        "~/bin",
        "~/.bin",
        "~/local/bin",
        // Nix user profiles and NixOS/nix-darwin system profiles.
        "~/.nix-profile/bin",
        "/run/current-system/sw/bin",
    ]);

    #[cfg(target_os = "macos")]
    paths.push("/opt/homebrew/bin");

    #[cfg(target_os = "linux")]
    paths.extend(["/usr/bin", "/home/linuxbrew/.linuxbrew/bin"]);

    #[cfg(target_os = "windows")]
    paths.extend([
        "%LOCALAPPDATA%/Microsoft/WinGet/Links",
        "%APPDATA%/npm",
        "%ProgramFiles%/nodejs",
        "~/scoop/shims",
        "%ProgramData%/chocolatey/bin",
    ]);

    let mut paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    paths.extend(extra_paths);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Some(user) = env_var_not_empty("USER") {
        paths.push(
            PathBuf::from("/etc/profiles/per-user")
                .join(user)
                .join("bin"),
        );
    }
    if let Some(path) = env_var_not_empty("PATH") {
        paths.extend(std::env::split_paths(&path));
    }

    #[cfg(target_os = "windows")]
    let filenames = [format!("{name}.exe"), format!("{name}.cmd")];
    #[cfg(not(target_os = "windows"))]
    let filenames = [name.to_owned()];

    for directory in paths {
        if directory.is_empty() {
            continue;
        }
        let directory = if let Ok(relative) = directory.strip_prefix("~") {
            let Some(home) = dirs::home_dir() else {
                continue;
            };
            home.join(relative)
        } else {
            #[cfg(target_os = "windows")]
            {
                if let Some((variable, relative)) = directory
                    .to_str()
                    .and_then(|path| path.strip_prefix('%'))
                    .and_then(|path| path.split_once("%/"))
                {
                    let Some(root) = env_var_not_empty(variable) else {
                        continue;
                    };
                    PathBuf::from(root).join(relative)
                } else {
                    directory
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                directory
            }
        };
        for filename in &filenames {
            let candidate = directory.join(filename);
            let metadata = match std::fs::metadata(&candidate) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to inspect {}", candidate.display()));
                }
            };
            if !metadata.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

/// Runs a command with an optional directory appended to the child's PATH and bounded execution time.
/// Must be called inside `spawn_blocking`.
pub(crate) fn command_output(
    program: &Path,
    args: &[&str],
    env_path: Option<PathBuf>,
) -> Result<Output> {
    let runtime =
        tokio::runtime::Handle::try_current().context("command_output requires a Tokio runtime")?;
    log::debug!("running command: {}, args: {args:?}", program.display());
    runtime.block_on(async {
        let mut command = tokio::process::Command::new(program);
        if let Some(env_path) = env_path {
            let path = env_var_not_empty("PATH").unwrap_or_default();
            let new_path = std::env::join_paths(
                std::env::split_paths(&path)
                    .filter(|path| !path.is_empty())
                    .chain(core::iter::once(env_path)),
            )
            .context("failed to construct CLI PATH")?;
            command.env("PATH", new_path);
        }
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .with_context(|| format!("{} timed out after 5 seconds", program.display()))?
            .with_context(|| format!("failed to run {}", program.display()))
    })
}
