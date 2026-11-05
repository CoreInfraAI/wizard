//! Applications that an installation closes, and opening them again afterwards.
//!
//! Wizard only asks applications to quit: an application that refuses or hangs keeps running,
//! and the installation then fails as if the user had not agreed to close it.

use super::Target;
#[cfg(target_os = "macos")]
use super::command_output;
#[cfg(target_os = "macos")]
use crate::platform::macos::{app_is_running, read_app_info, request_app_quit, running_app_paths};
#[cfg(target_os = "macos")]
use core::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::{process::Command, time::Instant};

/// How long an application may take to quit, including a confirmation it asks the user for.
/// The Windows and Linux installation scripts use the same limit.
#[cfg(target_os = "macos")]
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Products that installing `target` would close because they run in this user session now.
#[must_use]
pub fn running_apps(target: &Target) -> Vec<&'static str> {
    #[cfg(target_os = "macos")]
    let running = bundles(target)
        .into_iter()
        .filter(|(id, _)| app_is_running(id))
        .map(|(_, product)| product)
        .collect();
    #[cfg(target_os = "windows")]
    let running = if windows::package_running(&target.app.spec().package) {
        vec![target.app.product()]
    } else {
        Vec::new()
    };
    #[cfg(target_os = "linux")]
    let running = if target.app == super::DesktopApp::Claude && !linux::processes().is_empty() {
        vec![target.app.product()]
    } else {
        Vec::new()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let running = {
        let _ = target;
        Vec::new()
    };
    running
}

/// Bundle identifiers and products of the applications that installation replaces or moves.
#[cfg(target_os = "macos")]
fn bundles(target: &Target) -> Vec<(&'static str, &'static str)> {
    let mut bundles = vec![(target.app.spec().bundle.id, target.app.product())];
    if let Some(rename) = &target.rename {
        bundles.push((rename.bundle_id, rename.name));
    }
    bundles
}

/// Asks the applications that installing `target` replaces to quit, and waits until none runs or
/// until the timeout. The callers' running checks report what still runs. Returns what to open
/// again afterwards.
#[cfg(target_os = "macos")]
pub(super) fn quit(target: &Target) -> Reopen {
    let reopen = Reopen::capture(target);
    let bundles = bundles(target);
    for (id, _) in &bundles {
        log::info!("asking {id} to quit before installation");
        request_app_quit(id);
    }
    let deadline = Instant::now() + QUIT_TIMEOUT;
    while bundles.iter().any(|(id, _)| app_is_running(id)) {
        if Instant::now() >= deadline {
            log::warn!("applications did not quit before installation timed out");
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    reopen
}

/// What ran before installation, to open again once installation has closed it.
#[derive(Default)]
pub(super) struct Reopen {
    /// Bundle identifier, product, and the bundles its instances ran from.
    #[cfg(target_os = "macos")]
    bundles: Vec<(&'static str, &'static str, Vec<PathBuf>)>,
    #[cfg(target_os = "windows")]
    package: bool,
    /// Executable and arguments of the main process.
    #[cfg(target_os = "linux")]
    command: Option<(PathBuf, Vec<std::ffi::OsString>)>,
}

impl Reopen {
    pub(super) fn capture(target: &Target) -> Self {
        #[cfg(target_os = "macos")]
        let reopen = Self {
            bundles: bundles(target)
                .into_iter()
                .filter_map(|(id, product)| {
                    let paths = running_app_paths(id);
                    (!paths.is_empty()).then_some((id, product, paths))
                })
                .collect(),
        };
        #[cfg(target_os = "windows")]
        let reopen = Self {
            package: !running_apps(target).is_empty(),
        };
        #[cfg(target_os = "linux")]
        let reopen = Self {
            command: (target.app == super::DesktopApp::Claude)
                .then(linux::main_command)
                .flatten(),
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let reopen = {
            let _ = target;
            Self {}
        };
        reopen
    }

    /// Opens what ran before and no longer runs: the installed version, or the previous one when
    /// installation failed. Returns notices about applications that could not be opened.
    pub(super) fn run(self, target: &Target) -> Vec<String> {
        let mut notices = Vec::new();
        let mut report = |product: &str, error: &anyhow::Error| {
            log::error!("{product} could not be opened again after installation: {error:#}");
            notices.push(format!(
                "Не удалось снова открыть {product}: {error:#}. Откройте его вручную."
            ));
        };

        #[cfg(target_os = "macos")]
        for (id, product, paths) in self.bundles {
            if app_is_running(id) {
                continue;
            }
            // The installed bundle comes first; a failed installation leaves the previous one in
            // one of these places.
            let mut candidates: Vec<&std::path::Path> =
                target.path.iter().map(PathBuf::as_path).collect();
            if let Some(rename) = &target.rename {
                candidates.extend([rename.to.as_path(), rename.from.as_path()]);
            }
            candidates.extend(paths.iter().map(PathBuf::as_path));
            let found = candidates.into_iter().find(|path| {
                read_app_info(path).is_ok_and(|info| info.identifier.as_deref() == Some(id))
            });
            let result = match found {
                // Open in the background: Wizard keeps showing the installation result.
                Some(path) => command_output(
                    Command::new("/usr/bin/open").arg("-g").arg(path),
                    &format!("Не удалось открыть {}", path.display()),
                )
                .map(|_| log::info!("{product} opened again after installation")),
                None => Err(anyhow::anyhow!("приложение не найдено")),
            };
            if let Err(error) = result {
                report(product, &error);
            }
        }

        #[cfg(target_os = "windows")]
        if self.package && !windows::package_running(&target.app.spec().package) {
            match windows::open_package(&target.app.spec().package) {
                Ok(()) => log::info!("{} opened again after installation", target.app.product()),
                Err(error) => report(target.app.product(), &error),
            }
        }

        #[cfg(target_os = "linux")]
        if let Some((executable, arguments)) = self.command
            && linux::processes().is_empty()
        {
            match linux::open(&executable, &arguments) {
                Ok(()) => log::info!("{} opened again after installation", target.app.product()),
                Err(error) => report(target.app.product(), &error),
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let _ = (target, &mut report);
        notices
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::super::{WindowsPackage, command_output, powershell};
    use anyhow::Result;

    /// Whether the package's process runs in this user session. The installation script repeats
    /// the check with the package's exact location.
    #[expect(unsafe_code, reason = "Win32 process enumeration")]
    pub(super) fn package_running(package: &WindowsPackage) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                    TH32CS_SNAPPROCESS,
                },
                RemoteDesktop::ProcessIdToSessionId,
                Threading::GetCurrentProcessId,
            },
        };

        let executable = format!("{}.exe", package.process);
        // MSIX installs into <volume>\WindowsApps\<package full name>, which starts with the name.
        let location = format!("\\windowsapps\\{}_", package.name.to_lowercase());
        let mut session = 0;
        // SAFETY: the out pointer is valid for the call.
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &raw mut session) } == 0 {
            log::warn!("current Windows session could not be determined");
            return false;
        }
        // SAFETY: no pointers are passed; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            log::warn!("Windows process list could not be read");
            return false;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the structure is 568 bytes"
        )]
        let size = size_of::<PROCESSENTRY32W>() as u32;
        let mut entry = PROCESSENTRY32W {
            dwSize: size,
            ..Default::default()
        };
        let mut running = false;
        // SAFETY: `entry` is a valid PROCESSENTRY32W with its size set.
        let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) } != 0;
        while more && !running {
            let length = entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
            let mut process_session = 0;
            running = name.eq_ignore_ascii_case(&executable)
                // SAFETY: the out pointer is valid for the call.
                && unsafe { ProcessIdToSessionId(entry.th32ProcessID, &raw mut process_session) } != 0
                && process_session == session
                && (!package.process_in_package
                    || image_path(entry.th32ProcessID)
                        .is_some_and(|path| path.to_lowercase().contains(&location)));
            // SAFETY: as for Process32FirstW.
            more = unsafe { Process32NextW(snapshot, &raw mut entry) } != 0;
        }
        // SAFETY: the snapshot handle is valid and closed once.
        unsafe { CloseHandle(snapshot) };
        running
    }

    #[expect(unsafe_code, reason = "Win32 process queries")]
    fn image_path(process_id: u32) -> Option<String> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        };

        // SAFETY: no pointers are passed; the handle is closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0_u16; 32_768];
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the buffer holds 32768 units"
        )]
        let mut size = buffer.len() as u32;
        // SAFETY: `size` is the buffer's capacity in UTF-16 units.
        let read = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                buffer.as_mut_ptr(),
                &raw mut size,
            )
        } != 0;
        // SAFETY: the process handle is valid and closed once.
        unsafe { CloseHandle(process) };
        read.then(|| String::from_utf16_lossy(&buffer[..size as usize]))
    }

    /// Starts the package's application as the user, like its Start menu entry.
    pub(super) fn open_package(package: &WindowsPackage) -> Result<()> {
        let script = r"
$ErrorActionPreference = 'Stop'
$package = $env:WIZARD_DESKTOP_PACKAGE | ConvertFrom-Json
$installed = @(Get-AppxPackage -Name $package.name | Where-Object { $_.Publisher -ceq $package.publisher })
if ($installed.Count -eq 0) { exit 2 }
$application = @((Get-AppxPackageManifest -Package $installed[0].PackageFullName).Package.Applications.Application)[0]
if ($null -eq $application) { exit 3 }
Start-Process -FilePath ('shell:AppsFolder\' + $installed[0].PackageFamilyName + '!' + $application.Id)
";
        let parameters = serde_json::json!({
            "name": package.name,
            "publisher": package.publisher,
        })
        .to_string();
        command_output(
            powershell(script).env("WIZARD_DESKTOP_PACKAGE", parameters),
            "Windows не удалось запустить приложение",
        )
        .map(drop)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context as _, Result, ensure};
    use std::{
        ffi::OsString,
        fs,
        os::unix::ffi::{OsStrExt as _, OsStringExt as _},
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    /// Where the Claude Desktop package installs; the installation script checks the same place.
    const DIRECTORY: &str = "/usr/lib/claude-desktop";

    pub(super) struct Process {
        executable: PathBuf,
        command_line: Vec<u8>,
    }

    /// Claude Desktop processes that this user can inspect, which excludes other users'.
    pub(super) fn processes() -> Vec<Process> {
        let Ok(entries) = fs::read_dir("/proc") else {
            log::warn!("process list could not be read");
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().as_bytes().iter().all(u8::is_ascii_digit))
            .filter_map(|entry| {
                let executable = fs::read_link(entry.path().join("exe")).ok()?;
                executable.starts_with(DIRECTORY).then(|| Process {
                    executable,
                    command_line: fs::read(entry.path().join("cmdline")).unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Executable and arguments of the Electron main process.
    pub(super) fn main_command() -> Option<(PathBuf, Vec<OsString>)> {
        processes().iter().find_map(command)
    }

    /// Executable and arguments of a main process. Helper processes have `--type=`, possibly
    /// within one rewritten command-line string.
    fn command(process: &Process) -> Option<(PathBuf, Vec<OsString>)> {
        let helper = process
            .command_line
            .windows(7)
            .any(|window| window == b"--type=")
            || process
                .executable
                .file_name()
                .is_some_and(|name| name.as_bytes().starts_with(b"chrome"));
        if helper {
            return None;
        }
        // A replaced executable reads as "<path> (deleted)".
        let executable = process.executable.as_os_str().as_bytes();
        let executable = executable.strip_suffix(b" (deleted)").unwrap_or(executable);
        let arguments = process
            .command_line
            .strip_suffix(b"\0")
            .unwrap_or(&process.command_line)
            .split(|byte| *byte == 0)
            .skip(1)
            .map(|argument| OsString::from_vec(argument.to_vec()))
            .collect();
        Some((
            PathBuf::from(OsString::from_vec(executable.to_vec())),
            arguments,
        ))
    }

    /// Starts the application in its own session, not as Wizard's child.
    pub(super) fn open(executable: &Path, arguments: &[OsString]) -> Result<()> {
        let status = Command::new("/usr/bin/setsid")
            .arg("--fork")
            .arg(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Не удалось запустить приложение")?;
        ensure!(status.success(), "Не удалось запустить приложение");
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn process(executable: &str, command_line: &[u8]) -> Process {
            Process {
                executable: PathBuf::from(executable),
                command_line: command_line.to_vec(),
            }
        }

        #[test]
        fn main_process_command_excludes_helpers() {
            let main = process(
                "/usr/lib/claude-desktop/claude (deleted)",
                b"/usr/lib/claude-desktop/claude\0--ozone-platform-hint=auto\0\0",
            );
            assert_eq!(
                command(&main),
                Some((
                    PathBuf::from("/usr/lib/claude-desktop/claude"),
                    vec![
                        OsString::from("--ozone-platform-hint=auto"),
                        OsString::new()
                    ],
                ))
            );
            let renderer = process(
                "/usr/lib/claude-desktop/claude",
                b"/usr/lib/claude-desktop/claude --type=renderer --lang=en",
            );
            assert_eq!(command(&renderer), None);
            let crashpad = process(
                "/usr/lib/claude-desktop/chrome_crashpad_handler",
                b"/usr/lib/claude-desktop/chrome_crashpad_handler\0--monitor-self\0",
            );
            assert_eq!(command(&crashpad), None);
        }
    }
}
