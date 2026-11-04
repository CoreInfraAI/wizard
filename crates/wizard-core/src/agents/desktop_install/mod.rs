//! Desktop application release checks, downloads and installation. No settings writes or UI state.

mod chatgpt;
mod claude;

use super::TokenAgent;
#[cfg(target_os = "macos")]
use crate::platform::macos::{app_is_running, read_app_version};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::time::Duration;
use reqwest::{Client, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

const MAX_DOWNLOAD: u64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_NAME: &str = if cfg!(target_os = "windows") {
    "download.msix"
} else if cfg!(target_os = "linux") {
    "download.deb"
} else {
    "download.zip"
};

/// Desktop application that Wizard installs and updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopApp {
    Claude,
    /// The `ChatGPT` app with `ChatGPT` Work and Codex (`com.openai.codex`), not `ChatGPT` Classic.
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

impl DesktopApp {
    pub const ALL: [Self; 2] = [Self::Claude, Self::ChatGpt];

    /// Short name for user messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.spec().name
    }

    /// Product name for user messages and logs.
    #[must_use]
    pub const fn product(self) -> &'static str {
        self.spec().product
    }

    /// Hub token that authorizes this application's downloads through `CoreInfra`.
    #[must_use]
    pub const fn token_agent(self) -> TokenAgent {
        match self {
            Self::Claude => TokenAgent::ClaudeDesktop,
            Self::ChatGpt => TokenAgent::Codex,
        }
    }

    /// Why Wizard cannot install this application on this computer, if it cannot.
    #[must_use]
    pub fn unsupported_reason(self) -> Option<String> {
        match self {
            Self::Claude => claude::unsupported_reason(),
            Self::ChatGpt => chatgpt::unsupported_reason(),
        }
    }

    const fn spec(self) -> &'static AppSpec {
        match self {
            Self::Claude => &claude::SPEC,
            Self::ChatGpt => &chatgpt::SPEC,
        }
    }
}

struct AppSpec {
    /// Short name in user messages.
    name: &'static str,
    /// Product name in user messages and logs.
    product: &'static str,
    #[cfg(target_os = "macos")]
    bundle: MacBundle,
    #[cfg(target_os = "windows")]
    package: WindowsPackage,
}

#[cfg(target_os = "macos")]
struct MacBundle {
    file_name: &'static str,
    id: &'static str,
    /// The vendor's Apple-issued Developer ID, not merely any valid code signature.
    requirement: &'static str,
    /// Developer named in signature errors.
    vendor: &'static str,
}

#[cfg(target_os = "windows")]
struct WindowsPackage {
    name: &'static str,
    /// Process name that must not run during installation.
    process: &'static str,
    /// Count only processes started from the installed package: another application can share
    /// the process name.
    process_in_package: bool,
    publisher: &'static str,
    /// Signer named in signature errors.
    signer: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    app: DesktopApp,
    /// Installation path. Windows and Linux package managers choose installation locations.
    pub path: Option<PathBuf>,
    /// Installed version that a release must exceed, possibly of a superseded copy.
    version: Option<Version>,
    /// `path` holds the installed application, which installation replaces in place.
    replaces: bool,
    /// Another application at `path`, moved aside after explicit confirmation.
    rename: Option<Rename>,
    /// Older copies of the application, moved to the Trash after installation.
    superseded: Vec<PathBuf>,
}

impl Target {
    #[must_use]
    pub const fn app(&self) -> DesktopApp {
        self.app
    }

    fn new(app: DesktopApp, path: Option<PathBuf>, version: Option<Version>) -> Self {
        Self {
            app,
            path,
            replaces: version.is_some(),
            version,
            rename: None,
            superseded: Vec::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Rename {
    from: PathBuf,
    to: PathBuf,
    /// Bundle identifier of the application being moved, which must not be running.
    bundle_id: &'static str,
    name: &'static str,
}

#[derive(Clone)]
pub struct Release {
    pub version: Version,
    /// Version as the application reports it; a Windows package version can differ.
    pub display_version: String,
    pub target: Target,
    url: Url,
    integrity: Integrity,
    /// Archive size stated by the feed; otherwise the download response must state it.
    size: Option<u64>,
}

#[derive(Clone)]
enum Integrity {
    /// SHA-256 of the archive, published by the feed.
    Sha256(String),
    /// Sparkle `EdDSA` signature of the archive, made with the application's own update key.
    Ed25519 {
        signature: ed25519_dalek::Signature,
        key: &'static str,
    },
    /// The MSIX signature, checked against the publisher during installation.
    PackageSignature,
}

impl Release {
    /// Installation path to show, when Wizard chooses it.
    #[must_use]
    pub fn path(&self) -> Option<String> {
        self.target
            .path
            .as_ref()
            .map(|path| path.display().to_string())
    }

    /// Whether an existing installation, possibly a superseded copy, is updated.
    #[must_use]
    pub const fn updates_installation(&self) -> bool {
        self.target.version.is_some()
    }

    /// What installation changes besides the application itself.
    #[must_use]
    pub fn notes(&self) -> Vec<String> {
        self.target
            .superseded
            .iter()
            .map(|path| {
                format!(
                    "После установки {} будет перемещён в Корзину. Ярлыки в Dock и объекты входа, указывающие на него, перестанут работать.",
                    path.display()
                )
            })
            .collect()
    }

    /// A change that requires the user's explicit confirmation before installation.
    #[must_use]
    pub fn confirmation(&self) -> Option<String> {
        self.target.rename.as_ref().map(|rename| {
            format!(
                "Переименовать {} ({}) в {}, чтобы установить {} на его место",
                rename.name,
                rename.from.display(),
                rename.to.display(),
                self.target.app.name(),
            )
        })
    }
}

pub struct DownloadedRelease {
    directory: TempDir,
    pub release: Release,
    /// SHA-256 of the verified download. Installation checks the file again against it.
    sha256: String,
}

/// Completed installation.
#[derive(Debug)]
pub struct Installed {
    /// What the user should know or do next.
    pub notices: Vec<String>,
}

/// Finds what an installation of `app` would update, replace or create.
/// `last_path` is the application's last detected location.
pub fn detect_target(app: DesktopApp, last_path: Option<&Path>) -> Result<Target> {
    match app {
        DesktopApp::Claude => claude::detect_target(last_path),
        DesktopApp::ChatGpt => chatgpt::detect_target(),
    }
}

/// Returns a release newer than `target`, if the vendor publishes one for this computer.
pub async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
    match target.app {
        DesktopApp::Claude => claude::check_release(token, target).await,
        DesktopApp::ChatGpt => chatgpt::check_release(token, target).await,
    }
}

fn client(token: &str) -> Result<Client> {
    ensure!(!token.is_empty(), "Выберите токен CoreInfra Hub");
    let proxy = reqwest::Proxy::all(format!(
        "https://{}:{}",
        wizard_proxy::UPSTREAM_HOST,
        wizard_proxy::UPSTREAM_PORT
    ))
    .map_err(|_| anyhow!("Не удалось настроить прокси"))?
    .basic_auth(wizard_proxy::UPSTREAM_USERNAME, token);

    Client::builder()
        .user_agent(wizard_proxy::user_agent())
        .no_proxy()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        // Archives reach 1 GB: detect stalls without limiting the whole download.
        .read_timeout(Duration::from_mins(1))
        .build()
        .map_err(|_| anyhow!("Не удалось создать HTTP-клиент"))
}

/// Fetches a small update feed, refusing responses over `limit` bytes.
async fn fetch_feed(client: &Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| {
            anyhow!("Не удалось проверить обновления через прокси; проверьте токен и соединение")
        })?;
    ensure!(
        response.status().is_success(),
        "Сервер проверки обновлений вернул HTTP {}",
        response.status().as_u16()
    );

    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Не удалось прочитать ответ сервера"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Ответ сервера слишком большой"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Accepts only an HTTPS URL on `host`, without credentials or a custom port, whose path starts
/// with `prefix` and ends with `extension`.
fn download_url(text: &str, host: &str, prefix: &str, extension: &str) -> Result<Url> {
    let url = Url::parse(text).context("Некорректный адрес архива")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.path().starts_with(prefix)
            && url.path().ends_with(extension)
            && !url
                .path()
                .split('/')
                .any(|part| part == ".." || part == "."),
        "Недопустимый адрес архива"
    );
    Ok(url)
}

/// Downloads and verifies the release archive. Failed verification discards it.
pub async fn download_release(
    release: Release,
    token: &str,
    mut progress: impl FnMut(u64, u64),
) -> Result<DownloadedRelease> {
    let name = release.target.app.name();
    let directory = tempfile::Builder::new()
        .prefix(&format!("wizard-{}-", name.to_ascii_lowercase()))
        .tempdir()
        .context("Не удалось создать временный каталог")?;
    let mut file = fs::File::create(directory.path().join(DOWNLOAD_NAME))?;

    let mut response = client(token)?
        .get(release.url.clone())
        .send()
        .await
        .map_err(|_| {
            anyhow!("Не удалось скачать {name} через прокси; проверьте токен и соединение")
        })?;
    ensure!(
        response.status().is_success(),
        "Сервер загрузки вернул HTTP {}",
        response.status().as_u16()
    );

    let size = match release.size {
        Some(size) => size,
        None => response
            .content_length()
            .context("Сервер загрузки не сообщил размер архива")?,
    };
    ensure!(
        (1..=MAX_DOWNLOAD).contains(&size),
        "Недопустимый размер архива"
    );
    if let Some(length) = response.content_length() {
        ensure!(length == size, "Размер архива не совпадает с заявленным");
    }

    let mut verifier = ArchiveVerifier::new(&release.integrity, name)?;
    let mut received = 0;
    let mut published = 0;
    progress(0, size);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Загрузка {name} прервана"))?
    {
        received += chunk.len() as u64;
        ensure!(received <= size, "Размер архива превышает заявленный");
        file.write_all(&chunk)
            .context("Не удалось записать архив")?;
        verifier.update(&chunk);
        if received - published >= 1024 * 1024 || received == size {
            progress(received, size);
            published = received;
        }
    }
    ensure!(received == size, "Архив загружен не полностью");
    file.sync_all().context("Не удалось записать архив")?;

    let sha256 = verifier.finish(&release.integrity, name)?;
    Ok(DownloadedRelease {
        directory,
        release,
        sha256,
    })
}

/// Hashes an archive as it streams in and checks it against the feed.
struct ArchiveVerifier {
    hash: Sha256,
    signature: Option<ed25519_dalek::StreamVerifier>,
}

impl ArchiveVerifier {
    fn new(integrity: &Integrity, name: &str) -> Result<Self> {
        let signature = match integrity {
            Integrity::Ed25519 { signature, key } => Some(
                ed25519_verifying_key(key)?
                    .verify_stream(signature)
                    .map_err(|_| anyhow!("Некорректная подпись архива {name}"))?,
            ),
            Integrity::Sha256(_) | Integrity::PackageSignature => None,
        };
        Ok(Self {
            hash: Sha256::new(),
            signature,
        })
    }

    fn update(&mut self, chunk: &[u8]) {
        self.hash.update(chunk);
        if let Some(signature) = &mut self.signature {
            signature.update(chunk);
        }
    }

    /// Returns the archive's SHA-256 once it matches the feed.
    fn finish(self, integrity: &Integrity, name: &str) -> Result<String> {
        let sha256 = format!("{:x}", self.hash.finalize());
        match integrity {
            Integrity::Sha256(expected) => {
                ensure!(sha256 == *expected, "SHA-256 архива не совпадает");
            }
            Integrity::Ed25519 { .. } => self
                .signature
                .context("Отсутствует проверка подписи")?
                .finalize_and_verify()
                .map_err(|_| anyhow!("Подпись архива {name} не прошла проверку"))?,
            // Installation checks the package signature before deployment.
            Integrity::PackageSignature => {}
        }
        Ok(sha256)
    }
}

fn ed25519_verifying_key(key: &str) -> Result<ed25519_dalek::VerifyingKey> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(key)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .context("Некорректный ключ проверки обновлений")?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes)
        .map_err(|_| anyhow!("Некорректный ключ проверки обновлений"))
}

fn command_output(command: &mut Command, message: &str) -> Result<String> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context(message.to_owned())?;
    ensure!(output.status.success(), "{message}");
    String::from_utf8(output.stdout).context(message.to_owned())
}

/// Checks identity, version and the vendor's code signature, including nested code.
#[cfg(target_os = "macos")]
fn verify_bundle(spec: &AppSpec, path: &Path, version: &Version) -> Result<()> {
    let plist = plist::Value::from_file(path.join("Contents/Info.plist"))?;
    ensure!(
        plist
            .as_dictionary()
            .and_then(|p| p.get("CFBundleIdentifier"))
            .and_then(plist::Value::as_string)
            == Some(spec.bundle.id),
        "Неверный идентификатор приложения"
    );

    let actual = read_app_version(path)?.context("В приложении отсутствует версия")?;
    ensure!(
        Version::parse(&actual)? == *version,
        "Версия приложения не совпадает с выбранной"
    );

    command_output(
        Command::new("/usr/bin/codesign")
            .args([
                "--verify",
                "--deep",
                "--strict",
                "-R",
                spec.bundle.requirement,
            ])
            .arg(path),
        &format!(
            "Подпись {} не прошла проверку {}",
            spec.name, spec.bundle.vendor
        ),
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn powershell(script: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    // A pwsh -> Cargo -> powershell.exe chain inherits incompatible PowerShell 7 modules.
    // Let Windows PowerShell reconstruct its own default module paths.
    command.env_remove("PSModulePath");
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; only WINDOWS_ELEVATE requests UAC.
    command
}

// Only explicitly emitted diagnostics may reach the log. Raw PowerShell stderr
// can include invocation details and the complete script.
#[cfg_attr(
    not(any(target_os = "windows", test)),
    expect(dead_code, reason = "Windows installer diagnostic protocol")
)]
fn windows_install_diagnostics(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter_map(|line| line.strip_prefix("WIZARD_MSIX_ERROR: "))
}

#[cfg(target_os = "windows")]
const WINDOWS_IN_USE_DIAGNOSTICS: &str = r"
function Write-PackageUsageDiagnostic {
    try {
        # Resolve the installed, trusted package, not executable-name guesses.
        $installed = @(Get-AppxPackage -Name $package.name -ErrorAction Stop | Where-Object {
            $_.Name -ceq $package.name -and $_.Publisher -ceq $package.publisher
        })
        if ($installed.Count -ne 1 -or !$installed[0].InstallLocation) {
            Write-InstallMessage 'Related component snapshot unavailable: installed package identity not resolved'
            return
        }
        $root = [IO.Path]::GetFullPath($installed[0].InstallLocation).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        $all = @(Get-CimInstance -ClassName Win32_Process -OperationTimeoutSec 5 -ErrorAction Stop)
        $unavailable = @($all | Where-Object { !$_.ExecutablePath }).Count
        $related = @($all | Where-Object {
            if (!$_.ExecutablePath) { return $false }
            try {
                [IO.Path]::GetFullPath($_.ExecutablePath).StartsWith($root, [StringComparison]::OrdinalIgnoreCase)
            } catch { $false }
        })
        Write-InstallMessage ('Related component snapshot (not proven blockers): processes={0}, unavailable_executable_paths={1}' -f $related.Count, $unavailable)
        foreach ($process in ($related | Select-Object -First 16)) {
            Write-InstallMessage ('Related process: name={0} pid={1}' -f $process.Name, $process.ProcessId)
        }
        if ($related.Count -gt 16) { Write-InstallMessage 'Related process list truncated at 16 entries' }

        # Service ownership is linked through the verified executable's PID.
        # Do not parse or log service command lines, and never stop anything.
        $processIds = @($related | ForEach-Object { $_.ProcessId })
        $services = @(Get-CimInstance -ClassName Win32_Service -OperationTimeoutSec 5 -ErrorAction Stop | Where-Object {
            $_.ProcessId -gt 0 -and $processIds -contains $_.ProcessId
        })
        foreach ($service in ($services | Select-Object -First 16)) {
            Write-InstallMessage ('Related service: name={0} pid={1} state={2} start_mode={3}' -f $service.Name, $service.ProcessId, $service.State, $service.StartMode)
        }
        if ($services.Count -gt 16) { Write-InstallMessage 'Related service list truncated at 16 entries' }
    } catch {
        Write-InstallMessage ('Related component snapshot failed: HRESULT=0x{0:X8}' -f $_.Exception.HResult)
    }
}
function Report-InstallFailure($failure) {
    Write-InstallDiagnostic $failure
    $inUse = $false
    if ($phase -eq 16) {
        try {
            $exception = $failure.Exception
            for ($depth = 0; $null -ne $exception -and $depth -lt 16; $depth++) {
                # Appx can wrap the native HRESULT; its message then contains the deployment code.
                if (('{0:X8}' -f $exception.HResult) -eq '80073D02' -or $exception.Message -match '(?i)\b0x80073d02\b') {
                    $inUse = $true
                    break
                }
                $exception = $exception.InnerException
            }
        } catch {
            # Failure classification must not replace the original error.
        }
    }
    if ($inUse) {
        Write-InstallMessage 'Deployment resources in use: HRESULT=0x80073D02'
        Write-PackageUsageDiagnostic
        return 24
    }
    return $phase
}
";

#[cfg(target_os = "windows")]
fn windows_install_script(setup: &str, body: &str) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$phase = 1
function Write-InstallMessage($message) {{
    try {{
        $message = $message.Replace("`r", '').Replace("`n", '\n')
        if ($message.Length -gt 4096) {{ $message = $message.Substring(0, 4096) }}
        $text = 'WIZARD_MSIX_ERROR: ' + $message
        if ($diagnosticPath) {{
            [IO.File]::AppendAllText($diagnosticPath, $text + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
        }} else {{
            [Console]::Error.WriteLine($text)
        }}
    }} catch {{
        # Diagnostic failures must not replace the original exit code.
    }}
}}
function Write-InstallDiagnostic($failure) {{
    try {{
        # Do not format ErrorRecord, InvocationInfo, command arguments or environment.
        Write-InstallMessage ('phase={{0}} HRESULT=0x{{1:X8}} {{2}}' -f $phase, $failure.Exception.HResult, $failure.Exception.Message)
    }} catch {{}}
}}
{WINDOWS_IN_USE_DIAGNOSTICS}
try {{
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    {setup}
    {body}
}} catch {{
    exit (Report-InstallFailure $_)
}}
"#
    )
}

#[cfg(target_os = "windows")]
const WINDOWS_INSTALL: &str = r"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
function Test-PackageRunning {
    $processes = @(Get-Process -Name $package.process -ErrorAction SilentlyContinue)
    if (!$package.process_in_package -or $processes.Count -eq 0) { return $processes.Count -gt 0 }
    # Another application can use the same process name; count only this package's processes.
    $installed = @(Get-AppxPackage -Name $package.name -ErrorAction SilentlyContinue | Where-Object {
        $_.Publisher -ceq $package.publisher -and $_.InstallLocation
    })
    if ($installed.Count -eq 0) { return $false }
    $root = [IO.Path]::GetFullPath($installed[0].InstallLocation).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    foreach ($process in $processes) {
        try {
            if ($process.Path -and [IO.Path]::GetFullPath($process.Path).StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { return $true }
        } catch {}
    }
    return $false
}
$phase = 18
$lock = $null
try {
    if ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -cne $package.sid) { exit 18 }
    $phase = 22
    # Deny writes/replacement until deployment finishes, including across verification.
    $lock = [IO.File]::Open($package.archive, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $phase = 23
    # Hash the locked stream without reopening the path through PowerShell's file provider.
    if ((Get-FileHash -InputStream $lock -Algorithm SHA256).Hash -ne $package.sha256) { exit 20 }
    $phase = 10
    $signature = Get-AuthenticodeSignature -LiteralPath $package.archive
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -cne $package.publisher) { exit 10 }

    $phase = 11
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($package.archive)
    try {
        $entry = $zip.GetEntry('AppxManifest.xml')
        if ($null -eq $entry -or $entry.Length -gt 1048576) { exit 11 }
        $settings = New-Object Xml.XmlReaderSettings
        $settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
        $settings.XmlResolver = $null
        $stream = $entry.Open()
        try {
            $reader = [Xml.XmlReader]::Create($stream, $settings)
            try {
                $manifest = New-Object Xml.XmlDocument
                $manifest.Load($reader)
            } finally { $reader.Dispose() }
        } finally { $stream.Dispose() }
    } finally { $zip.Dispose() }
    $identity = $manifest.DocumentElement.Identity
    if ($identity.Name -cne $package.name -or $identity.Publisher -cne $package.publisher) { exit 12 }
    if ($identity.Version -cne $package.version) { exit 13 }
    $arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    if ($arch -notin @('x64', 'arm64') -or $identity.ProcessorArchitecture -cne $arch) { exit 14 }

    $phase = 15
    if (Test-PackageRunning) { exit 15 }
    if (!$install) { exit 0 }
    $phase = 16
    # Let deployment close remaining package components; no deferred registration or downgrade.
    Add-AppxPackage -Path $package.archive -ForceTargetApplicationShutdown | Out-Null
    $phase = 17
    $installed = Get-AppxPackage -Name $package.name
    if ($null -eq $installed -or $installed.Publisher -cne $package.publisher -or $installed.Version.ToString() -cne $package.version) { exit 17 }
    exit 0
} catch {
    exit (Report-InstallFailure $_)
}
finally { if ($null -ne $lock) { $lock.Dispose() } }
";

#[cfg(target_os = "windows")]
const WINDOWS_ELEVATE: &str = r#"
# Only public package metadata, the diagnostic path and originating SID cross elevation.
$phase = 21
try {
    $json = ($package | ConvertTo-Json -Compress).Replace("'", "''")
    $path = ''
    if ($package.diagnostic_path) { $path = $package.diagnostic_path.Replace("'", "''") }
    $script = '$diagnosticPath = ''' + $path + '''; $packageJson = ''' + $json + ''';' + [Environment]::NewLine + $body
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
    $child = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -Verb RunAs -WindowStyle Hidden -Wait -PassThru -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', $encoded)
    if ($child.ExitCode -ne 0) { exit $child.ExitCode }
    # Confirm registration from the original, unelevated user's context too.
    $installed = Get-AppxPackage -Name $package.name
    if ($null -eq $installed -or $installed.Publisher -cne $package.publisher -or $installed.Version.ToString() -cne $package.version) { exit 17 }
    exit 0
} catch {
    Write-InstallDiagnostic $_
    if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) { exit 19 }
    exit 21
}
"#;

#[cfg(target_os = "windows")]
fn install_windows(spec: &AppSpec, archive: &Path, version: &Version, sha256: &str) -> Result<()> {
    let name = spec.name;
    ensure!(
        version.pre.is_empty()
            && version.build.is_empty()
            && [version.major, version.minor, version.patch]
                .into_iter()
                .all(|part| u16::try_from(part).is_ok()),
        "Версия {name} не соответствует формату MSIX"
    );
    let diagnostics = if let Ok(directory) = tempfile::tempdir() {
        Some(directory)
    } else {
        log::warn!("Failed to prepare {name} MSIX installation diagnostic directory");
        None
    };
    let diagnostic_path = diagnostics
        .as_ref()
        .map(|directory| directory.path().join("install-error.txt"));

    log::warn!(
        "{name} MSIX installation uses ForceTargetApplicationShutdown to close remaining package components"
    );

    let package = serde_json::json!({
        "diagnostic_path": diagnostic_path,
        "archive": archive,
        "name": spec.package.name,
        "process": spec.package.process,
        "process_in_package": spec.package.process_in_package,
        "publisher": spec.package.publisher,
        "version": format!("{version}.0"),
        "sha256": sha256,
        "sid": "",
    })
    .to_string();
    let setup = r"
$diagnosticPath = $null
$package = $env:WIZARD_DESKTOP_PACKAGE | ConvertFrom-Json
$package.sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
";
    // Reject corrupt packages and a running application before asking for UAC, even in
    // force-close mode.
    run_windows_install(
        spec,
        powershell(&windows_install_script(
            &format!("{setup}\n$install = $false"),
            WINDOWS_INSTALL,
        ))
        .env("WIZARD_DESKTOP_PACKAGE", &package),
        "preflight",
        None,
    )?;
    let elevated = windows_install_script(
        "$package = $packageJson | ConvertFrom-Json\n$install = $true",
        WINDOWS_INSTALL,
    );
    run_windows_install(
        spec,
        powershell(&windows_install_script(
            setup,
            &format!("$body = @'\n{elevated}\n'@\n{WINDOWS_ELEVATE}"),
        ))
        .env("WIZARD_DESKTOP_PACKAGE", &package),
        "elevation",
        diagnostic_path.as_deref(),
    )
}

#[cfg(target_os = "windows")]
fn run_windows_install(
    spec: &AppSpec,
    command: &mut Command,
    stage: &'static str,
    diagnostic_path: Option<&Path>,
) -> Result<()> {
    let name = spec.name;
    log::info!("{name} MSIX {stage} started");
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Не удалось выполнить PowerShell установки MSIX ({stage})"))
        .inspect_err(|error| {
            log::error!("{name} MSIX {stage} process failed: {error:#}");
        })?;
    if output.status.success() {
        log::info!("{name} MSIX {stage} completed");
        return Ok(());
    }

    log::error!(
        "{name} MSIX {stage} failed: exit code {:?}",
        output.status.code()
    );
    let mut detailed = false;
    for diagnostic in windows_install_diagnostics(&String::from_utf8_lossy(&output.stderr)) {
        log::error!("{name} MSIX {stage}: {diagnostic}");
        detailed = true;
    }
    if let Some(path) = diagnostic_path {
        match fs::File::open(path) {
            Ok(file) => {
                let mut text = String::new();
                if file.take(64 * 1024).read_to_string(&mut text).is_ok() {
                    for diagnostic in windows_install_diagnostics(&text) {
                        log::error!("{name} MSIX {stage}: {diagnostic}");
                        detailed = true;
                    }
                } else {
                    log::warn!("{name} MSIX {stage}: diagnostic file could not be read as UTF-8");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!(
                "{name} MSIX {stage}: diagnostic file could not be opened ({:?}, OS code {:?})",
                error.kind(),
                error.raw_os_error()
            ),
        }
    }
    if !detailed {
        log::warn!(
            "{name} MSIX {stage}: no exception diagnostic received; raw PowerShell output is omitted"
        );
    }
    let message = match output.status.code() {
        Some(10) => format!("Подпись MSIX не прошла проверку {}", spec.package.signer),
        Some(11) => "Не удалось прочитать манифест MSIX".to_owned(),
        Some(12) => format!("Неверный идентификатор или издатель пакета {name}"),
        Some(13) => "Версия MSIX не совпадает с выбранной".to_owned(),
        Some(14) => "Архитектура MSIX не соответствует Windows".to_owned(),
        Some(15) => format!(
            "Полностью закройте {}, включая значок в трее, и повторите установку",
            spec.product
        ),
        Some(17) => format!(
            "Windows завершила установку, но ожидаемая версия {name} не обнаружена у исходного пользователя"
        ),
        Some(18) => "В UAC выбрана другая учётная запись. Подтвердите повышение прав для того же пользователя, который запустил Wizard".to_owned(),
        Some(19) => "Запрос прав администратора отменён. Пакет сохранён для повторной установки".to_owned(),
        Some(20) => "SHA-256 пакета MSIX не совпадает".to_owned(),
        Some(21) => "Не удалось запустить процесс установки с правами администратора".to_owned(),
        Some(22) => "Не удалось открыть и заблокировать пакет MSIX".to_owned(),
        Some(23) => "Не удалось вычислить SHA-256 пакета MSIX".to_owned(),
        Some(24) => format!(
            "Обновление {name} заблокировано: ресурсы пакета используются (0x80073D02). Закрытия окна или значка в трее может быть недостаточно. Сведения о связанных процессах и службах записаны в лог, если их удалось получить. Пакет сохранён для повторной попытки"
        ),
        _ => "Windows не удалось установить MSIX. Проверьте зависимости пакета и ограничения системы. Пакет сохранён для повторной попытки".to_owned(),
    };
    log::error!("{name} MSIX {stage}: {message}");
    bail!("{message}")
}

fn archive_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).context("Не удалось открыть архив для проверки SHA-256")?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .context("Не удалось прочитать архив для проверки SHA-256")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Installs a downloaded release. `confirmed` approves the release's `confirmation`.
/// The archive stays for a retry when installation fails.
pub fn install_release(download: &DownloadedRelease, confirmed: bool) -> Result<Installed> {
    let product = download.release.target.app.product();
    log::info!(
        "{product} installation started: version {}",
        download.release.version
    );
    let result = install_release_inner(download, confirmed);
    match &result {
        Ok(_) => log::info!(
            "{product} installation completed: version {}",
            download.release.version
        ),
        Err(error) => log::error!(
            "{product} installation failed: version {}: {error:#}",
            download.release.version
        ),
    }
    result
}

fn install_release_inner(download: &DownloadedRelease, confirmed: bool) -> Result<Installed> {
    let archive = download.directory.path().join(DOWNLOAD_NAME);
    ensure!(
        archive_sha256(&archive)? == download.sha256,
        "SHA-256 архива не совпадает"
    );

    #[cfg(target_os = "macos")]
    let mut notices = install_macos(download, confirmed)?;
    #[cfg(not(target_os = "macos"))]
    let mut notices = {
        ensure!(
            download.release.target.rename.is_none() || confirmed,
            "Подтвердите переименование приложения"
        );
        Vec::new()
    };
    #[cfg(target_os = "windows")]
    install_windows(
        download.release.target.app.spec(),
        &archive,
        &download.release.version,
        &download.sha256,
    )?;
    #[cfg(target_os = "linux")]
    match download.release.target.app {
        DesktopApp::Claude => claude::install_linux(&archive, &download.release, &download.sha256)?,
        DesktopApp::ChatGpt => bail!("ChatGPT не выпускается для этой системы"),
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    bail!(
        "Установка {} на этой платформе ещё не реализована",
        download.release.target.app.product()
    );

    let target = &download.release.target;
    // A new ChatGPT creates its profile on first launch; routing can be applied only then.
    if target.app == DesktopApp::ChatGpt && !target.replaces && target.superseded.is_empty() {
        notices.push(
            "Откройте ChatGPT один раз, затем нажмите «Применить» в разделе «Подключение», чтобы подключить его к CoreInfra."
                .to_owned(),
        );
    }
    Ok(Installed { notices })
}

/// Replaces the bundle, moving aside a confirmed foreign bundle first and superseded copies to
/// the Trash afterwards. Returns notices about the moved bundles.
#[cfg(target_os = "macos")]
fn install_macos(download: &DownloadedRelease, confirmed: bool) -> Result<Vec<String>> {
    let DownloadedRelease {
        directory, release, ..
    } = download;
    let spec = release.target.app.spec();
    let archive = directory.path().join(DOWNLOAD_NAME);
    let extracted = tempfile::Builder::new()
        .prefix("extracted-")
        .tempdir_in(directory.path())
        .context("Не удалось создать каталог распаковки")?;
    command_output(
        Command::new("/usr/bin/tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(extracted.path()),
        &format!("Не удалось распаковать {}", spec.name),
    )?;

    let app = extracted.path().join(spec.bundle.file_name);
    let destination = release
        .target
        .path
        .as_ref()
        .context("Не указан путь установки")?;
    let parent = destination
        .parent()
        .context("Некорректный путь установки")?;
    let staging = tempfile::Builder::new()
        .prefix(&format!(".wizard-{}-", spec.name.to_ascii_lowercase()))
        .tempdir_in(parent)
        .context("Нет прав на установку в каталог приложения")?;
    let staged = staging.path().join(spec.bundle.file_name);
    command_output(
        Command::new("/usr/bin/ditto").arg(&app).arg(&staged),
        "Не удалось подготовить приложение к замене",
    )?;
    verify_bundle(spec, &staged, &release.version)?;

    let installed = detect_target(release.target.app, release.target.path.as_deref())?;
    ensure!(
        installed == release.target,
        "{} изменился после проверки обновления; проверьте обновление снова",
        spec.product
    );
    ensure!(
        !app_is_running(spec.bundle.id),
        "Полностью закройте {}, включая значок в меню, и повторите установку",
        spec.product
    );

    let mut notices = Vec::new();
    let renamed = match &release.target.rename {
        Some(rename) => {
            ensure!(
                confirmed,
                "Подтвердите переименование {}, чтобы продолжить установку",
                rename.name
            );
            ensure!(
                !app_is_running(rename.bundle_id),
                "Закройте {} и повторите установку",
                rename.name
            );
            ensure!(
                !rename.to.exists(),
                "{} уже существует; переименуйте или удалите его и повторите проверку",
                rename.to.display()
            );
            fs::rename(&rename.from, &rename.to)
                .with_context(|| format!("Не удалось переименовать {}", rename.name))?;
            Some(rename)
        }
        None => None,
    };
    if let Err(error) = replace_bundle(staging, &staged, &release.target) {
        if let Some(rename) = renamed
            && let Err(restore) = fs::rename(&rename.to, &rename.from)
        {
            log::error!("{} rename could not be undone: {restore}", rename.name);
            return Err(error.context(format!(
                "{} остался переименован в {}",
                rename.name,
                rename.to.display()
            )));
        }
        return Err(error);
    }
    if let Some(rename) = renamed {
        notices.push(format!(
            "{} переименован в {}.",
            rename.name,
            rename.to.display()
        ));
    }

    for path in &release.target.superseded {
        match retire(&spec.bundle, path) {
            Ok(()) => notices.push(format!(
                "{} перемещён в Корзину. Обновите ярлыки в Dock и объекты входа, если они указывали на него.",
                path.display()
            )),
            Err(error) => {
                log::warn!("superseded bundle was not moved to the Trash: {error:#}");
                notices.push(format!(
                    "Не удалось переместить {} в Корзину: {error:#}. Удалите его вручную.",
                    path.display()
                ));
            }
        }
    }
    Ok(notices)
}

/// Moves an older copy of the application to the Trash, only if its developer signed it.
#[cfg(target_os = "macos")]
fn retire(bundle: &MacBundle, path: &Path) -> Result<()> {
    command_output(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", "-R", bundle.requirement])
            .arg(path),
        &format!("Приложение не подписано {}", bundle.vendor),
    )?;
    // The application can start again while the installation finishes.
    ensure!(
        !app_is_running(bundle.id),
        "Приложение запущено; закройте его"
    );
    crate::platform::macos::move_to_trash(path)
}

#[cfg(target_os = "macos")]
fn replace_bundle(staging: TempDir, staged: &Path, target: &Target) -> Result<()> {
    let name = target.app.name();
    let destination = target.path.as_ref().context("Не указан путь установки")?;
    let previous = staging.path().join("previous.app");
    if target.replaces {
        fs::rename(destination, &previous)
            .with_context(|| format!("Не удалось переместить предыдущую версию {name}"))?;
    }
    if let Err(error) = fs::rename(staged, destination) {
        if target.replaces && fs::rename(&previous, destination).is_err() {
            let preserved = staging.keep();
            bail!(
                "Замена и восстановление {name} не удались. Предыдущая версия сохранена в {}",
                preserved.join("previous.app").display()
            );
        }
        return Err(error)
            .with_context(|| format!("Не удалось установить {name}; предыдущая версия сохранена"));
    }
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "bounded test fixtures")]
mod tests {
    use super::*;
    use serde_json::json;

    std::thread_local! {
        static CAPTURED_LOGS: core::cell::RefCell<Option<Vec<String>>> = const {
            core::cell::RefCell::new(None)
        };
    }

    struct TestLogger;

    impl log::Log for TestLogger {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            CAPTURED_LOGS.with(|logs| {
                if let Some(lines) = logs.borrow_mut().as_mut() {
                    lines.push(format!("{} {}", record.level(), record.args()));
                }
            });
        }

        fn flush(&self) {}
    }

    fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
        static LOGGER: TestLogger = TestLogger;
        static INITIALIZE: std::sync::Once = std::sync::Once::new();
        INITIALIZE.call_once(|| {
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(log::LevelFilter::Info);
        });
        CAPTURED_LOGS.with(|logs| assert!(logs.replace(Some(Vec::new())).is_none()));
        let result = run();
        let text = CAPTURED_LOGS.with(|logs| logs.take().unwrap().join("\n"));
        (result, text)
    }

    fn target(version: Option<&str>) -> Target {
        Target::new(
            DesktopApp::Claude,
            Some(PathBuf::from("Claude.app")),
            version.map(|v| Version::parse(v).unwrap()),
        )
    }

    fn feed() -> serde_json::Value {
        json!({"currentRelease":"3.2.1", "releases":[{"version":"3.2.1", "updateTo":{
            "version":"3.2.1", "url":"https://downloads.claude.ai/releases/darwin/arm64/3.2.1/Claude.zip",
            "sha256":"a".repeat(64), "size":1024
        }}]})
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn failed_install_keeps_archive_for_retry_and_cleans_extraction() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join(DOWNLOAD_NAME);
        fs::write(&archive, b"invalid zip").unwrap();
        let mut release =
            claude::parse_release(&serde_json::to_vec(&feed()).unwrap(), target(None))
                .unwrap()
                .unwrap();
        // No personal application path is used, even if extraction unexpectedly succeeds.
        release.target.path = Some(directory.path().join("destination.app"));
        let download = DownloadedRelease {
            sha256: archive_sha256(&archive).unwrap(),
            directory,
            release,
        };

        for _ in 0..2 {
            let error = install_release(&download, false).unwrap_err();
            assert_eq!(
                error.to_string(),
                if cfg!(target_os = "windows") {
                    "Подпись MSIX не прошла проверку Anthropic"
                } else if cfg!(target_os = "linux") {
                    "Не удалось прочитать пакет DEB"
                } else {
                    "Не удалось распаковать Claude"
                }
            );
            assert_eq!(fs::read(&archive).unwrap(), b"invalid zip");
            assert_eq!(fs::read_dir(download.directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn archive_verifier_checks_signature_and_hash_while_streaming() {
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;

        let signing = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let key: &'static str = Box::leak(
            base64::engine::general_purpose::STANDARD
                .encode(signing.verifying_key().as_bytes())
                .into_boxed_str(),
        );
        let archive = b"archive bytes".repeat(1000);
        let mut tampered = archive.clone();
        tampered[5] ^= 1;
        let sha256 = format!("{:x}", Sha256::digest(&archive));
        let verify = |integrity: &Integrity, data: &[u8]| {
            let mut verifier = ArchiveVerifier::new(integrity, "ChatGPT").unwrap();
            for chunk in data.chunks(777) {
                verifier.update(chunk);
            }
            verifier.finish(integrity, "ChatGPT")
        };

        let signed = Integrity::Ed25519 {
            signature: signing.sign(&archive),
            key,
        };
        assert_eq!(verify(&signed, &archive).unwrap(), sha256);
        assert_eq!(
            verify(&signed, &tampered).unwrap_err().to_string(),
            "Подпись архива ChatGPT не прошла проверку"
        );

        let hashed = Integrity::Sha256(sha256.clone());
        assert_eq!(verify(&hashed, &archive).unwrap(), sha256);
        assert_eq!(
            verify(&hashed, &tampered).unwrap_err().to_string(),
            "SHA-256 архива не совпадает"
        );
        // MSIX signatures are checked at installation; the hash still binds the file to it.
        assert_eq!(
            verify(&Integrity::PackageSignature, &archive).unwrap(),
            sha256
        );
    }

    #[test]
    fn archive_failures_are_logged_before_native_installation() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join(DOWNLOAD_NAME);
        fs::write(&archive, b"invalid archive").unwrap();
        let release = claude::parse_release(&serde_json::to_vec(&feed()).unwrap(), target(None))
            .unwrap()
            .unwrap();
        let download = DownloadedRelease {
            directory,
            release,
            sha256: "a".repeat(64),
        };

        let (result, logs) = capture_logs(|| install_release(&download, false));
        assert!(result.is_err());
        assert!(logs.contains("ERROR Claude Desktop installation failed: version 3.2.1"));
        assert!(logs.contains("SHA-256 архива не совпадает"));
        assert!(!logs.contains("installation completed"));
        assert!(archive.exists());

        fs::remove_file(&archive).unwrap();
        let (result, logs) = capture_logs(|| install_release(&download, false));
        assert!(result.is_err());
        assert!(logs.contains("ERROR Claude Desktop installation failed"));
        assert!(logs.contains("Не удалось открыть архив для проверки SHA-256"));
    }

    #[test]
    fn windows_diagnostics_exclude_raw_powershell_errors() {
        let text = "At line:1\n+ command payload\nWIZARD_MSIX_ERROR: phase=16 HRESULT=0x80073CF3 dependency failure\nInvocationInfo: command payload\n";
        assert_eq!(
            windows_install_diagnostics(text).collect::<Vec<_>>(),
            ["phase=16 HRESULT=0x80073CF3 dependency failure"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_diagnostics_preserve_errors_without_elevation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("error.txt");
        let failure = "throw [InvalidOperationException]::new('synthetic installation failure: ошибка установки')";
        let run = |script: &str, path: &Path, stage: &'static str| {
            let (result, logs) = capture_logs(|| {
                run_windows_install(
                    &claude::SPEC,
                    powershell(script).env("WIZARD_TEST_DIAGNOSTIC", path),
                    stage,
                    (stage == "elevation").then_some(path),
                )
            });
            assert!(result.is_err());
            assert!(logs.contains(&format!("ERROR Claude MSIX {stage}")));
            assert!(!logs.contains("throw [InvalidOperationException]"));
            logs
        };

        let (result, logs) = capture_logs(|| {
            run_windows_install(
                &claude::SPEC,
                &mut Command::new(directory.path().join("missing-powershell.exe")),
                "preflight",
                None,
            )
        });
        assert!(result.is_err());
        assert!(logs.contains("ERROR Claude MSIX preflight process failed"));

        // A setup exception must be reported without dumping the script.
        let logs = run(&windows_install_script(failure, ""), &path, "preflight");
        assert!(logs.contains("exit code Some(1)"));
        assert!(logs.contains("HRESULT=0x80131509"));
        assert!(logs.contains("synthetic installation failure: ошибка установки"));

        let logs = run("exit 13", &path, "preflight");
        assert!(logs.contains("Версия MSIX не совпадает с выбранной"));
        let logs = run(
            "[Console]::Error.WriteLine('RAW_SCRIPT_PAYLOAD'); exit 1",
            &path,
            "preflight",
        );
        assert!(logs.contains("no exception diagnostic received"));
        assert!(!logs.contains("RAW_SCRIPT_PAYLOAD"));

        // The same reporter writes the elevated channel; failure to write must
        // neither replace the original exit code nor leak raw stderr.
        let script = windows_install_script(
            "$diagnosticPath = $env:WIZARD_TEST_DIAGNOSTIC\n$phase = 16",
            failure,
        );
        let logs = run(&script, &path, "elevation");
        assert!(logs.contains("exit code Some(16)"));
        assert!(logs.contains("phase=16"));
        assert!(logs.contains("ошибка установки"));
        let logs = run(&script, directory.path(), "elevation");
        assert!(logs.contains("exit code Some(16)"));
        assert!(logs.contains("no exception diagnostic received"));

        // Replace RunAs with a hidden, unelevated child. Exercise the actual
        // script handoff and exit-code propagation, never UAC or deployment.
        let mock = r"
function Start-Process {
    param($FilePath, $Verb, $WindowStyle, [switch]$Wait, [switch]$PassThru, $ArgumentList)
    $child = [Diagnostics.Process]::new()
    $child.StartInfo.FileName = $FilePath
    $child.StartInfo.Arguments = $ArgumentList -join ' '
    $child.StartInfo.UseShellExecute = $false
    $child.StartInfo.CreateNoWindow = $true
    $child.StartInfo.RedirectStandardOutput = $true
    $child.StartInfo.RedirectStandardError = $true
    try {
        $child.Start() | Out-Null
        $child.BeginOutputReadLine()
        $child.BeginErrorReadLine()
        if (!$child.WaitForExit(10000)) { $child.Kill(); throw 'fixture timeout' }
        [pscustomobject]@{ ExitCode = $child.ExitCode }
    } finally { $child.Dispose() }
}
";
        fs::remove_file(&path).unwrap();
        let child = windows_install_script(
            "$package = $packageJson | ConvertFrom-Json\n$phase = 16",
            failure,
        );
        let setup = "$package = @{ diagnostic_path = $env:WIZARD_TEST_DIAGNOSTIC }";
        let script = windows_install_script(
            setup,
            &format!("{mock}\n$body = @'\n{child}\n'@\n{WINDOWS_ELEVATE}"),
        );
        let logs = run(&script, &path, "elevation");
        assert!(logs.contains("exit code Some(16)"));
        assert!(logs.contains("phase=16"));
        assert!(logs.contains("ошибка установки"));

        fs::remove_file(&path).unwrap();
        let cancellation =
            "function Start-Process { throw [ComponentModel.Win32Exception]::new(1223) }";
        let script = windows_install_script(
            setup,
            &format!("{cancellation}\n$body = ''\n{WINDOWS_ELEVATE}"),
        );
        let logs = run(&script, &path, "elevation");
        assert!(logs.contains("exit code Some(19)"));
        assert!(logs.contains("HRESULT=0x"));
        assert!(logs.contains("Запрос прав администратора отменён"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_in_use_diagnostics_match_package_paths() {
        // All package/process/service queries are fixtures, never host inspection.
        let fixture = r"
$package = @{ name = 'Claude'; publisher = 'fixture publisher' }
function Get-AppxPackage {
    [pscustomobject]@{ Name = 'Claude'; Publisher = 'fixture publisher'; InstallLocation = 'C:\Fixture\Claude' }
}
function Get-CimInstance {
    param($ClassName)
    if ($ClassName -eq 'Win32_Process') {
        [pscustomobject]@{ Name = 'cowork-svc.exe'; ProcessId = 101; ExecutablePath = 'C:\Fixture\Claude\cowork-svc.exe'; CommandLine = 'RAW_COMMAND_PAYLOAD' }
        [pscustomobject]@{ Name = 'claude-ssh-helper.exe'; ProcessId = 102; ExecutablePath = 'c:\fixture\CLAUDE\bin\claude-ssh-helper.exe' }
        [pscustomobject]@{ Name = 'cowork-svc.exe'; ProcessId = 201; ExecutablePath = 'C:\Fixture\Claude-other\cowork-svc.exe' }
        [pscustomobject]@{ Name = 'Claude.exe'; ProcessId = 202; ExecutablePath = 'C:\Fixture\Claude\..\Other\Claude.exe' }
        [pscustomobject]@{ Name = 'unknown.exe'; ProcessId = 203; ExecutablePath = $null }
    } elseif ($ClassName -eq 'Win32_Service') {
        [pscustomobject]@{ Name = 'fixture-cowork'; ProcessId = 101; State = 'Running'; StartMode = 'Auto'; PathName = 'RAW_COMMAND_PAYLOAD' }
        [pscustomobject]@{ Name = 'unrelated-service'; ProcessId = 201; State = 'Running'; StartMode = 'Auto'; PathName = 'C:\Fixture\Claude\misleading.exe' }
        [pscustomobject]@{ Name = 'stopped-service'; ProcessId = 0; State = 'Stopped'; StartMode = 'Auto' }
    } else { throw 'unexpected fixture query' }
}
";
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage.txt");
        let run = |setup: &str, phase: u8, failure: &str| {
            // Each attempted install gets a new report; use the real file channel.
            if path.exists() {
                fs::remove_file(&path).unwrap();
            }
            let script = windows_install_script(
                &format!(
                    "{fixture}\n{setup}\n$diagnosticPath = $env:WIZARD_TEST_DIAGNOSTIC\n$phase = {phase}"
                ),
                failure,
            );
            let (result, logs) = capture_logs(|| {
                run_windows_install(
                    &claude::SPEC,
                    powershell(&script).env("WIZARD_TEST_DIAGNOSTIC", &path),
                    "elevation",
                    Some(&path),
                )
            });
            assert!(result.is_err());
            assert!(!logs.contains("RAW_COMMAND_PAYLOAD"));
            (result.unwrap_err().to_string(), logs)
        };

        let native =
            "throw [Runtime.InteropServices.COMException]::new('resources busy', -2147009278)";
        let (message, logs) = run("", 16, native);
        assert!(message.contains("ресурсы пакета используются (0x80073D02)"));
        assert!(logs.contains("exit code Some(24)"));
        assert!(logs.contains("not proven blockers"));
        assert!(logs.contains("processes=2, unavailable_executable_paths=1"));
        assert!(logs.contains("name=cowork-svc.exe pid=101"));
        assert!(logs.contains("name=claude-ssh-helper.exe pid=102"));
        assert!(logs.contains("name=fixture-cowork pid=101 state=Running start_mode=Auto"));
        assert!(!logs.contains("pid=201"));
        assert!(!logs.contains("pid=202"));
        assert!(!logs.contains("unrelated-service"));
        assert!(!logs.contains("stopped-service"));

        let wrapped = "throw [InvalidOperationException]::new('Deployment failed: 0x80073d02')";
        assert!(run("", 16, wrapped).1.contains("exit code Some(24)"));
        let (_, logs) = run("", 17, wrapped);
        assert!(logs.contains("exit code Some(17)"));
        assert!(!logs.contains("Related component snapshot"));

        let (_, logs) = run(
            "function Get-CimInstance { throw 'probe failed' }",
            16,
            native,
        );
        assert!(logs.contains("exit code Some(24)"));
        assert!(logs.contains("HRESULT=0x80073D02"));
        assert!(logs.contains("Related component snapshot failed"));

        let foreign = "function Get-AppxPackage { [pscustomobject]@{ Name='Claude'; Publisher='foreign'; InstallLocation='C:\\Fixture\\Claude' } }";
        let (_, logs) = run(foreign, 16, native);
        assert!(logs.contains("exit code Some(24)"));
        assert!(logs.contains("installed package identity not resolved"));
        assert!(!logs.contains("Related process:"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn replacement_restores_old_bundle_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("Claude.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old"), "old").unwrap();
        let target = Target::new(
            DesktopApp::Claude,
            Some(destination.clone()),
            Some(Version::new(1, 0, 0)),
        );

        let staging = tempfile::tempdir_in(root.path()).unwrap();
        let missing = staging.path().join("missing.app");

        assert!(replace_bundle(staging, &missing, &target).is_err());
        assert_eq!(fs::read_to_string(destination.join("old")).unwrap(), "old");
    }
}
