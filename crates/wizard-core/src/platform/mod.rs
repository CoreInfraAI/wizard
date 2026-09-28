use core::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::{Context as _, Result};

#[cfg(target_os = "macos")]
pub(crate) mod macos;

/// Finds an executable by its bare name in PATH.
/// On macOS/Linux, must be called inside `spawn_blocking` with a Tokio runtime.
/// On Windows, checks `.exe` before `.cmd` in each PATH directory.
pub(crate) fn find_executable(name: &str) -> Result<Option<PathBuf>> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let found = command_output(Path::new("/usr/bin/which"), &[name])?;
        if found.status.code() == Some(1) {
            return Ok(None);
        }
        anyhow::ensure!(
            found.status.success(),
            "which {name} exited with {}: {}",
            found.status,
            String::from_utf8_lossy(&found.stderr).trim()
        );
        let path = core::str::from_utf8(&found.stdout)
            .with_context(|| format!("which {name} returned an invalid path"))?
            .trim_end_matches(['\r', '\n']);
        anyhow::ensure!(!path.is_empty(), "which {name} returned an invalid path");
        Ok(Some(PathBuf::from(path)))
    }
    #[cfg(target_os = "windows")]
    {
        let Some(path) = std::env::var_os("PATH") else {
            return Ok(None);
        };
        let filenames = [format!("{name}.exe"), format!("{name}.cmd")];
        for directory in std::env::split_paths(&path) {
            // Do not implicitly search the working directory.
            if directory.as_os_str().is_empty() {
                continue;
            }
            for filename in &filenames {
                let candidate = directory.join(filename);
                match std::fs::metadata(&candidate) {
                    Ok(metadata) if metadata.is_file() => return Ok(Some(candidate)),
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to inspect {name} in PATH"));
                    }
                }
            }
        }
        Ok(None)
    }
}

/// Runs a command from a Tokio blocking worker with bounded execution time.
/// Must be called inside `spawn_blocking`.
pub(crate) fn command_output(program: &Path, args: &[&str]) -> Result<Output> {
    let runtime =
        tokio::runtime::Handle::try_current().context("command_output requires a Tokio runtime")?;
    log::debug!("running command: {}, args: {args:?}", program.display());
    runtime.block_on(async {
        let mut command = tokio::process::Command::new(program);
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
