use anyhow::{Context as _, Result};
use core::time::Duration;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Output,
};

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// Reads a nonempty environment variable, logging only its name when unavailable.
pub(crate) fn env_var_not_empty(name: &str) -> Option<OsString> {
    let value = std::env::var_os(name).filter(|value| !value.is_empty());
    if value.is_none() {
        log::debug!("environment variable {name} is unset or empty");
    }
    value
}

/// Appends directories to the command's PATH, using inherited PATH if not overridden.
/// Empty entries are skipped. Does not change the parent process environment.
pub(crate) fn append_command_path(
    command: &mut tokio::process::Command,
    paths: &[&Path],
) -> Result<()> {
    let path = match command.as_std().get_envs().find(|(key, _)| {
        *key == "PATH"
            || (cfg!(target_os = "windows") && key.as_encoded_bytes().eq_ignore_ascii_case(b"PATH"))
    }) {
        Some((_, value)) => value.map(OsString::from),
        None => env_var_not_empty("PATH"),
    }
    .unwrap_or_default();
    let path = std::env::join_paths(
        std::env::split_paths(&path)
            .chain(paths.iter().map(|path| path.to_path_buf()))
            .filter(|path| !path.is_empty()),
    )
    .context("failed to construct command PATH")?;
    command.env("PATH", path);
    Ok(())
}

/// Checks the last found executable first, then searches common directories,
/// extra directories, the per-user Nix profile, and PATH.
/// Expands a leading `~` path component. On Windows, also expands leading `%VAR%/`
/// and checks `.exe` before `.cmd` in each directory.
pub(crate) fn find_executable(
    name: &str,
    extra_paths: Vec<PathBuf>,
    last_found: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if let Some(path) = last_found
        && is_executable(path)?
    {
        return Ok(Some(std::path::absolute(path)?));
    }

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
            if is_executable(&candidate)? {
                return Ok(Some(std::path::absolute(candidate)?));
            }
        }
    }

    Ok(None)
}

fn is_executable(path: &Path) -> Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if !metadata.is_file() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Runs a prepared command with bounded execution time and no stdin.
/// Callers explicitly configure stdout/stderr (piped to capture, null to discard).
/// Must be called inside `spawn_blocking`.
pub(crate) fn command_output(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<Output> {
    let runtime =
        tokio::runtime::Handle::try_current().context("command_output requires a Tokio runtime")?;
    let program_name = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    log::debug!("running command: {program_name}");
    runtime.block_on(async {
        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .with_context(|| format!("failed to run {program_name}"))?;
        tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .with_context(|| {
                format!(
                    "{program_name} timed out after {} seconds; the operation may be partially completed",
                    timeout.as_secs()
                )
            })?
            .context("failed to wait for command output")
    })
}
