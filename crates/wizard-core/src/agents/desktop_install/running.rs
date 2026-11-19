//! Applications that an installation closes, opening them again afterwards, and opening a
//! first installation.
//!
//! Wizard only asks applications to quit: an application that refuses or hangs keeps running,
//! and the installation then fails as if the user had not agreed to close it.

#[cfg(target_os = "macos")]
use super::command_output;
use super::{DesktopApp, Target};
use crate::agents::{AgentDetection, detection};
#[cfg(target_os = "macos")]
use crate::platform::macos::{app_is_running, read_app_info, request_app_quit, running_app_paths};
use anyhow::Result;
#[cfg(target_os = "macos")]
use anyhow::ensure;
#[cfg(target_os = "macos")]
use core::time::Duration;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "macos")]
use std::time::Instant;

/// How long an application may take to quit, including a confirmation it asks the user for.
/// The Windows and Linux installation scripts use the same limit.
#[cfg(target_os = "macos")]
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration owns the full close/write/open cycle. Never force-quit an application.
/// A fresh installation opens only after its configuration has been saved successfully.
pub(crate) fn with_closed(
    app: DesktopApp,
    last_path: Option<&Path>,
    apply: impl FnOnce() -> Result<()>,
) -> Result<()> {
    run_closed(
        || close_existing(app, last_path),
        apply,
        || open_existing(app, last_path),
    )
}

fn run_closed(
    close: impl FnOnce() -> Result<bool>,
    apply: impl FnOnce() -> Result<()>,
    open: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let was_running = close()?;
    let applied = apply();
    // Restore an already running app after an error, but never launch a first installation
    // with incomplete settings. Preserve the configuration error if reopening also fails.
    if !was_running && applied.is_err() {
        return applied;
    }
    let opened = open();
    match (applied, opened) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => {
            Err(error.context("Настройки сохранены, но не удалось открыть приложение"))
        }
        (Err(error), Err(open_error)) => Err(error.context(format!(
            "Не удалось снова открыть приложение: {open_error:#}"
        ))),
    }
}

fn close_existing(app: DesktopApp, last_path: Option<&Path>) -> Result<bool> {
    #[cfg(not(target_os = "linux"))]
    let _ = last_path;
    #[cfg(target_os = "macos")]
    {
        let id = app.spec().bundle.id;
        let was_running = app_is_running(id);
        request_app_quit(id);
        let deadline = Instant::now() + QUIT_TIMEOUT;
        while app_is_running(id) {
            ensure!(
                Instant::now() < deadline,
                "{} не закрылся. Завершите текущую работу и повторите настройку.",
                app.product()
            );
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(was_running)
    }
    #[cfg(target_os = "windows")]
    {
        windows::close_package(&app.spec().package)
    }
    #[cfg(target_os = "linux")]
    {
        match app {
            DesktopApp::Claude => linux::close(app, linux::claude_executable),
            DesktopApp::ChatGpt => {
                let info =
                    match detection::detect_desktop(detection::DesktopAgent::Codex, last_path) {
                        AgentDetection::Found(info) => info,
                        AgentDetection::NotFound => anyhow::bail!("ChatGPT не найден"),
                        AgentDetection::Error(error) => {
                            anyhow::bail!("Не удалось найти ChatGPT: {error}")
                        }
                    };
                let executable = linux::chatgpt_executable(&info.path)?;
                linux::close(app, |path| {
                    let bytes = path.as_os_str().as_encoded_bytes();
                    bytes.strip_suffix(b" (deleted)").unwrap_or(bytes)
                        == executable.as_os_str().as_encoded_bytes()
                })
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!(
            "Перезапуск {} на этой платформе не поддерживается",
            app.product()
        )
    }
}

#[cfg(test)]
mod configuration_tests {
    use super::run_closed;
    use core::cell::RefCell;

    #[test]
    fn configuration_is_saved_before_first_launch_or_restart() -> anyhow::Result<()> {
        for was_running in [false, true] {
            let steps = RefCell::new(Vec::new());
            run_closed(
                || {
                    steps.borrow_mut().push("closed");
                    Ok(was_running)
                },
                || {
                    steps.borrow_mut().push("configured");
                    Ok(())
                },
                || {
                    steps.borrow_mut().push("opened");
                    Ok(())
                },
            )?;
            assert_eq!(*steps.borrow(), ["closed", "configured", "opened"]);
        }
        Ok(())
    }

    #[test]
    fn refused_shutdown_prevents_configuration_and_launch() {
        let result = run_closed(
            || anyhow::bail!("quit refused"),
            || panic!("must not change a running app's profile"),
            || panic!("must not open another instance"),
        );
        assert_eq!(result.expect_err("must fail").to_string(), "quit refused");
    }

    #[test]
    fn failed_configuration_only_reopens_a_previously_running_app() {
        for was_running in [false, true] {
            let mut opened = false;
            let result = run_closed(
                || Ok(was_running),
                || anyhow::bail!("configuration failed"),
                || {
                    opened = true;
                    Ok(())
                },
            );
            assert_eq!(opened, was_running);
            assert_eq!(
                result.expect_err("must fail").to_string(),
                "configuration failed"
            );
        }
    }

    #[test]
    fn launch_failure_does_not_claim_setup_completed() {
        let result = run_closed(|| Ok(false), || Ok(()), || anyhow::bail!("launch failed"));
        let message = format!("{:#}", result.expect_err("must fail"));
        assert!(message.contains("Настройки сохранены"));
        assert!(message.contains("launch failed"));
    }
}

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
            match windows::open_package(&target.app.spec().package, None) {
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

/// Opens a first installation in the foreground, where the user signs in next; an update opens
/// nothing beyond what [`Reopen`] restores. Call from a blocking worker once installation and
/// its follow-up work have succeeded. Returns a notice when the application could not be opened.
#[must_use]
pub fn open_first_install(target: &Target) -> Option<String> {
    if !target.first_install() {
        return None;
    }
    let product = target.app.product();
    match open_installed(target) {
        Ok(()) => {
            log::info!("{product} opened after its first installation");
            None
        }
        Err(error) => {
            log::error!("{product} could not be opened after its first installation: {error:#}");
            Some(format!(
                "Не удалось открыть {product}: {error:#}. Откройте его вручную."
            ))
        }
    }
}

/// Re-detects before launching: an installer target can name a future path or a different
/// application being replaced. Launch must use the same app that discovery showed in the UI.
/// Call from a Tokio blocking worker.
pub fn open_existing(app: DesktopApp, last_path: Option<&Path>) -> anyhow::Result<()> {
    let agent = match app {
        DesktopApp::Claude => detection::DesktopAgent::Claude,
        DesktopApp::ChatGpt => detection::DesktopAgent::Codex,
    };
    match detection::detect_desktop(agent, last_path) {
        AgentDetection::Found(info) => open_application(app, Some(&info.path)),
        AgentDetection::NotFound => {
            anyhow::bail!("Агент больше не найден. Вернитесь к выбору агента.")
        }
        AgentDetection::Error(error) => anyhow::bail!("Не удалось найти агента: {error}"),
    }
}

/// Opens the just-installed application, or brings it to the front when it already runs.
fn open_installed(target: &Target) -> anyhow::Result<()> {
    open_application(target.app, target.path.as_deref())
}

fn open_application(app: DesktopApp, path: Option<&Path>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let opened = {
        let _ = app;
        let path = path.ok_or_else(|| anyhow::anyhow!("не указан путь установки"))?;
        command_output(
            Command::new("/usr/bin/open").arg(path),
            &format!("Не удалось открыть {}", path.display()),
        )
        .map(drop)
    };
    #[cfg(target_os = "windows")]
    let opened = windows::open_package(&app.spec().package, path);
    #[cfg(target_os = "linux")]
    let opened = {
        let path = match path {
            Some(path) => path,
            None if app == DesktopApp::Claude => Path::new(linux::LAUNCHER),
            None => anyhow::bail!("Не указан путь приложения"),
        };
        linux::open(path, &[])
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let opened = {
        let _ = (app, path);
        anyhow::bail!("Запуск приложения на этой платформе не поддерживается")
    };
    opened
}

#[cfg(target_os = "windows")]
mod windows {
    use super::super::{WindowsPackage, command_output, powershell};
    use anyhow::Result;

    pub(super) fn close_package(package: &WindowsPackage) -> Result<bool> {
        let script = super::super::windows_install_script(
            "$package = $env:WIZARD_DESKTOP_PACKAGE | ConvertFrom-Json",
            &format!(
                "{}\n{}",
                super::super::WINDOWS_CLOSE,
                r"
$running = @(Get-PackageProcesses)
if (@($running | Where-Object { $_.SessionId -ne [Diagnostics.Process]::GetCurrentProcess().SessionId }).Count -gt 0) { exit 25 }
if ($running.Count -gt 0) { Close-PackageProcesses $running }
if ($running.Count -gt 0) { 'running' } else { 'stopped' }
exit 0
"
            ),
        );
        let parameters = serde_json::json!({
            "name": package.name,
            "publisher": package.publisher,
            "process": package.process,
            "process_in_package": package.process_in_package,
        });
        let mut command = powershell(&script);
        command.env("WIZARD_DESKTOP_PACKAGE", parameters.to_string());
        let mut command = tokio::process::Command::from(command);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let output = crate::platform::command_output(command, core::time::Duration::from_secs(45))?;
        anyhow::ensure!(
            output.status.success(),
            "Приложение не закрылось. Завершите текущую работу и повторите настройку."
        );
        match output.stdout.trim_ascii() {
            b"running" => Ok(true),
            b"stopped" => Ok(false),
            _ => anyhow::bail!("Не удалось проверить закрытие приложения"),
        }
    }

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
    pub(super) fn open_package(
        package: &WindowsPackage,
        detected: Option<&std::path::Path>,
    ) -> Result<()> {
        let script = r"
$ErrorActionPreference = 'Stop'
$package = $env:WIZARD_DESKTOP_PACKAGE | ConvertFrom-Json
$names = @($package.name)
if ($package.path -and $package.name -eq 'Claude') { $names += 'AnthropicPBC.Claude' }
$installed = @(foreach ($name in $names) { Get-AppxPackage -Name $name | Where-Object { $_.Publisher -ceq $package.publisher } })
foreach ($candidate in ($installed | Sort-Object Version -Descending)) {
    $applications = @((Get-AppxPackageManifest -Package $candidate.PackageFullName).Package.Applications.Application)
    foreach ($application in $applications) {
        if ($package.path -and (Join-Path $candidate.InstallLocation $application.Executable) -ine $package.path) { continue }
        Start-Process -FilePath ('shell:AppsFolder\' + $candidate.PackageFamilyName + '!' + $application.Id)
        exit 0
    }
}
exit 2
";
        let parameters = serde_json::json!({
            "name": package.name,
            "publisher": package.publisher,
            "path": detected,
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
    /// What the package's menu entry runs, as `Exec=claude-desktop %U`.
    pub(super) const LAUNCHER: &str = "/usr/bin/claude-desktop";

    pub(super) struct Process {
        handle: Option<fs::File>,
        executable: PathBuf,
        command_line: Vec<u8>,
    }

    fn real_uid(status: &str) -> Option<u32> {
        status.lines().find_map(|line| {
            line.strip_prefix("Uid:")?
                .split_ascii_whitespace()
                .next()?
                .parse()
                .ok()
        })
    }

    /// Inspect only Claude processes with the same real UID as Wizard. A readable
    /// executable link alone does not establish ownership (e.g. a privileged GUI).
    pub(super) fn processes() -> Vec<Process> {
        find_current_user_app_processes(&claude_executable).unwrap_or_else(|_| {
            log::warn!("process list could not be read");
            Vec::new()
        })
    }

    pub(super) fn claude_executable(path: &Path) -> bool {
        path.starts_with(DIRECTORY)
    }

    pub(super) fn chatgpt_executable(launcher: &Path) -> Result<PathBuf> {
        use std::io::Read as _;

        let launcher = fs::canonicalize(launcher)?;
        let executable = chatgpt_binary_path(&launcher);
        let mut magic = [0; 4];
        ensure!(
            executable
                .file_name()
                .is_some_and(|name| name.as_bytes().eq_ignore_ascii_case(b"chatgpt"))
                && fs::File::open(executable)?.read_exact(&mut magic).is_ok()
                && magic == *b"\x7fELF",
            "Не удалось определить отдельный исполняемый файл ChatGPT по его лаунчеру"
        );
        Ok(executable.to_owned())
    }

    fn chatgpt_binary_path(launcher: &Path) -> &Path {
        // The DEB's /usr/bin/chatgpt resolves to this wrapper, not the Electron binary.
        if launcher == Path::new("/usr/lib/chatgpt/codex-launcher") {
            Path::new("/usr/lib/chatgpt/ChatGPT")
        } else {
            launcher
        }
    }

    fn find_current_user_app_processes(matches: &impl Fn(&Path) -> bool) -> Result<Vec<Process>> {
        use std::os::fd::AsRawFd as _;

        let uid = real_uid(&fs::read_to_string("/proc/self/status")?)
            .context("Не удалось определить пользователя Wizard")?;
        let mut found = Vec::new();
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            if !entry.file_name().as_bytes().iter().all(u8::is_ascii_digit) {
                continue;
            }
            // Fast filter, then pin the process and repeat identification via the handle.
            if !fs::read_link(entry.path().join("exe")).is_ok_and(|exe| matches(&exe)) {
                continue;
            }
            let inspect = || -> std::io::Result<Option<Process>> {
                let handle = fs::File::open(entry.path())?;
                let dir = PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
                let executable = fs::read_link(dir.join("exe"))?;
                if !matches(&executable)
                    || real_uid(&fs::read_to_string(dir.join("status"))?) != Some(uid)
                {
                    return Ok(None);
                }
                Ok(Some(Process {
                    handle: Some(handle),
                    executable,
                    command_line: fs::read(dir.join("cmdline"))?,
                }))
            };
            match inspect() {
                Ok(Some(process)) => found.push(process),
                Ok(None) => {}
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) => {
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(found)
    }

    pub(super) fn close(app: super::DesktopApp, matches: impl Fn(&Path) -> bool) -> Result<bool> {
        use core::time::Duration;
        use std::time::Instant;

        let processes = find_current_user_app_processes(&matches)?;
        if processes.is_empty() {
            return Ok(false);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        for process in &processes {
            if command(process).is_some() {
                request_quit(
                    process
                        .handle
                        .as_ref()
                        .context("Нет дескриптора процесса")?,
                )?;
            }
        }
        while !find_current_user_app_processes(&matches)?.is_empty() {
            ensure!(
                Instant::now() < deadline,
                "{} не закрылся. Завершите текущую работу и повторите попытку.",
                app.product()
            );
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(true)
    }

    fn request_quit(handle: &fs::File) -> Result<()> {
        use rustix::process::{Signal, pidfd_send_signal};

        match pidfd_send_signal(handle, Signal::TERM) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(error) => Err(error).context("Не удалось запросить закрытие приложения"),
        }
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
        if helper || process.command_line.is_empty() {
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
                handle: None,
                executable: PathBuf::from(executable),
                command_line: command_line.to_vec(),
            }
        }

        #[test]
        fn chatgpt_launcher_must_resolve_to_a_dedicated_binary() -> Result<()> {
            use std::os::unix::fs::symlink;

            let dir = tempfile::tempdir()?;
            let launcher = dir.path().join("chatgpt");
            let binary = dir.path().join("ChatGPT");
            fs::write(&binary, b"\x7fELF")?;
            symlink(&binary, &launcher)?;
            assert_eq!(chatgpt_executable(&launcher)?, fs::canonicalize(&binary)?);
            fs::write(&binary, b"#!/bin/sh")?;
            assert!(chatgpt_executable(&launcher).is_err());
            fs::remove_file(&launcher)?;
            let electron = dir.path().join("electron");
            fs::write(&electron, b"\x7fELF")?;
            symlink(electron, &launcher)?;
            assert!(chatgpt_executable(&launcher).is_err());
            Ok(())
        }

        #[test]
        fn quit_signals_only_the_pinned_test_child() -> Result<()> {
            use std::os::unix::process::ExitStatusExt as _;

            let mut child = Command::new("/usr/bin/sleep").arg("2").spawn()?;
            let handle = fs::File::open(format!("/proc/{}", child.id()))?;
            let sent = request_quit(&handle);
            let status = child.wait()?;
            sent?;
            assert_eq!(
                status.signal(),
                Some(rustix::process::Signal::TERM.as_raw())
            );
            // The original process is gone; the same descriptor remains harmless.
            request_quit(&handle)
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
