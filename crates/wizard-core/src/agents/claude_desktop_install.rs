//! Claude release checks, downloads and installation. No settings writes or UI state.

use super::{
    AgentDetection,
    detection::{self, DesktopAgent},
};
#[cfg(target_os = "macos")]
use crate::platform::macos::{app_is_running, os_version, read_app_version};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::time::Duration;
use reqwest::{Client, Url};
use semver::Version;
use serde::Deserialize;
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

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const DOWNLOAD_NAME: &str = if cfg!(target_os = "windows") {
    "download.msix"
} else if cfg!(target_os = "linux") {
    "download.deb"
} else {
    "download.zip"
};
#[cfg(target_os = "macos")]
const FEED: &str = "https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update";
// Apple-issued Developer ID of Anthropic, not merely any valid code signature.
#[cfg(target_os = "macos")]
const SIGNING_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.anthropic.claudefordesktop\" and certificate leaf[subject.OU] = \"Q6L2SF6YDW\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";

#[derive(Clone)]
pub struct Target {
    // Windows and Linux package managers choose installation locations.
    pub path: Option<PathBuf>,
    version: Option<Version>,
}

#[derive(Clone)]
pub struct Release {
    pub version: Version,
    pub target: Target,
    url: Url,
    sha256: String,
    size: u64,
}

pub struct DownloadedRelease {
    directory: TempDir,
    pub release: Release,
}

pub fn detect_target(last_path: Option<&Path>) -> Result<Target> {
    let (path, version) = match detection::detect_desktop(DesktopAgent::Claude, last_path) {
        AgentDetection::Found(info) => {
            // let version = info
            //     .version
            //     .context("Не удалось определить установленную версию Claude")?;
            let version = "4.21.0".to_owned();

            (
                if cfg!(any(target_os = "windows", target_os = "linux")) {
                    None
                } else {
                    Some(info.path)
                },
                Some(Version::parse(&version).context("Неизвестный формат версии Claude")?),
            )
        }
        AgentDetection::NotFound => {
            #[cfg(target_os = "macos")]
            {
                (Some(PathBuf::from(detection::CLAUDE_APP_PATH)), None)
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                (None, None)
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
            bail!("Установка Claude Desktop на этой платформе не поддерживается")
        }
        AgentDetection::Error(message) => bail!("{message}"),
    };

    Ok(Target { path, version })
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
        .timeout(Duration::from_mins(20))
        .build()
        .map_err(|_| anyhow!("Не удалось создать HTTP-клиент"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Feed {
    current_release: String,
    releases: Vec<FeedRelease>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedRelease {
    version: String,
    update_to: Payload,
}

#[derive(Deserialize)]
struct Payload {
    version: String,
    url: String,
    sha256: String,
    size: u64,
}

fn parse_release(bytes: &[u8], target: Target) -> Result<Option<Release>> {
    let feed: Feed =
        serde_json::from_slice(bytes).context("Некорректный ответ сервера обновлений")?;
    let version =
        Version::parse(&feed.current_release).context("Некорректная версия обновления")?;
    if target
        .version
        .as_ref()
        .is_some_and(|current| version <= *current)
    {
        return Ok(None);
    }

    let payload = feed
        .releases
        .into_iter()
        .find(|entry| entry.version == feed.current_release)
        .context("В ответе отсутствует архив указанной версии")?
        .update_to;
    validate_payload(version, payload, target).map(Some)
}

fn validate_payload(version: Version, mut payload: Payload, target: Target) -> Result<Release> {
    ensure!(
        payload.version == version.to_string(),
        "Версии в ответе сервера не совпадают"
    );

    let url = Url::parse(&payload.url).context("Некорректный адрес архива")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("downloads.claude.ai")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "Недопустимый адрес архива Claude"
    );

    ensure!(
        payload.sha256.len() == 64 && payload.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Некорректный SHA-256 архива"
    );
    ensure!(
        (1..=MAX_DOWNLOAD).contains(&payload.size),
        "Недопустимый размер архива"
    );

    payload.sha256.make_ascii_lowercase();
    Ok(Release {
        version,
        target,
        url,
        sha256: payload.sha256,
        size: payload.size,
    })
}

fn feed_url(target: &Target) -> Result<Url> {
    let (mut url, os_version) = platform_feed(target)?;
    url.query_pairs_mut()
        .append_pair("device_id", &uuid::Uuid::new_v4().to_string())
        .append_pair(
            "version",
            &target
                .version
                .as_ref()
                .map_or_else(|| "0.0.0".into(), ToString::to_string),
        )
        .append_pair("os_version", &os_version);
    Ok(url)
}

#[cfg(target_os = "macos")]
fn platform_feed(_target: &Target) -> Result<(Url, String)> {
    Ok((Url::parse(FEED)?, os_version()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_feed(_target: &Target) -> Result<(Url, String)> {
    bail!("Проверка обновлений Claude Desktop на этой платформе ещё не реализована")
}

pub async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
    if cfg!(target_os = "linux") {
        return check_linux_release(token, target).await;
    }
    let url = feed_url(&target)?;
    let mut response = client(token)?
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| {
            anyhow!("Не удалось проверить обновления через прокси; проверьте токен и соединение")
        })?;
    if matches!(response.status().as_u16(), 204 | 404) {
        ensure!(
            target.version.is_some(),
            "Сервер не предложил версию для установки"
        );
        return Ok(None);
    }
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
            bytes.len() + chunk.len() <= 1024 * 1024,
            "Ответ сервера слишком большой"
        );
        bytes.extend_from_slice(&chunk);
    }

    parse_release(&bytes, target)
}

pub async fn download_release(
    release: Release,
    token: &str,
    mut progress: impl FnMut(u64, u64),
) -> Result<DownloadedRelease> {
    let directory = tempfile::Builder::new()
        .prefix("wizard-claude-")
        .tempdir()
        .context("Не удалось создать временный каталог")?;
    let mut file = fs::File::create(directory.path().join(DOWNLOAD_NAME))?;

    let mut response = client(token)?
        .get(release.url.clone())
        .send()
        .await
        .map_err(|_| {
            anyhow!("Не удалось скачать Claude через прокси; проверьте токен и соединение")
        })?;
    ensure!(
        response.status().is_success(),
        "Сервер загрузки вернул HTTP {}",
        response.status().as_u16()
    );

    let mut received = 0;
    let mut published = 0;
    progress(0, release.size);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Загрузка Claude прервана"))?
    {
        received += chunk.len() as u64;
        ensure!(
            received <= release.size,
            "Размер архива превышает заявленный"
        );
        file.write_all(&chunk)
            .context("Не удалось записать архив")?;
        if received - published >= 1024 * 1024 || received == release.size {
            progress(received, release.size);
            published = received;
        }
    }

    ensure!(received == release.size, "Архив загружен не полностью");
    Ok(DownloadedRelease { directory, release })
}

const APT_ORIGIN: &str = "https://downloads.claude.ai/claude-desktop";
const APT_SIGNER: &str = "31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE";

async fn apt_metadata(client: &Client, path: &str) -> Result<Vec<u8>> {
    let mut response = client
        .get(format!("{APT_ORIGIN}/{path}"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| anyhow!("Не удалось получить метаданные APT через CoreInfra"))?;
    ensure!(
        response.status().is_success(),
        "Сервер APT вернул HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Не удалось прочитать метаданные APT"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 2 * 1024 * 1024,
            "Метаданные APT слишком велики"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn deb_field<'a>(paragraph: &'a str, name: &str) -> Result<&'a str> {
    paragraph
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)
                .and_then(|value| value.strip_prefix(": "))
        })
        .context("В метаданных APT отсутствует обязательное поле")
}

fn verify_apt_index(release: &str, arch: &str, index: &[u8]) -> Result<()> {
    let valid_until = deb_field(release, "Valid-Until")?.replace(" UTC", " +0000");
    let expiry = chrono::DateTime::parse_from_rfc2822(&valid_until)
        .context("Некорректный срок действия метаданных APT")?;
    ensure!(
        expiry > chrono::Utc::now(),
        "Подписанные метаданные APT устарели"
    );
    let path = format!("main/binary-{arch}/Packages");
    let entry = release
        .lines()
        .skip_while(|line| *line != "SHA256:")
        .skip(1)
        .take_while(|line| line.starts_with(' '))
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| fields.len() == 3 && fields[2] == path)
        .context("В подписанных метаданных нет SHA-256 индекса APT")?;
    ensure!(
        entry[1].parse::<usize>()? == index.len()
            && entry[0].eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(index))),
        "Размер или SHA-256 индекса APT не совпадает"
    );
    Ok(())
}

fn parse_apt_release(index: &str, arch: &str, target: Target) -> Result<Option<Release>> {
    let mut latest: Option<(Version, &str)> = None;
    for paragraph in index.split("\n\n") {
        if deb_field(paragraph, "Package").ok() != Some("claude-desktop")
            || deb_field(paragraph, "Architecture").ok() != Some(arch)
        {
            continue;
        }
        let version = Version::parse(deb_field(paragraph, "Version")?)
            .context("Некорректная версия Claude в APT")?;
        if latest
            .as_ref()
            .is_none_or(|(current, _)| version > *current)
        {
            latest = Some((version, paragraph));
        }
    }
    let (version, paragraph) = latest.context("В APT нет Claude для этой архитектуры")?;
    if target
        .version
        .as_ref()
        .is_some_and(|current| version <= *current)
    {
        return Ok(None);
    }
    let filename = deb_field(paragraph, "Filename")?;
    ensure!(
        filename.starts_with("pool/")
            && Path::new(filename)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("deb"))
            && !filename.split('/').any(|part| part == ".."),
        "Некорректный путь пакета APT"
    );
    let payload = Payload {
        version: version.to_string(),
        url: format!("{APT_ORIGIN}/apt/stable/{filename}"),
        sha256: deb_field(paragraph, "SHA256")?.to_owned(),
        size: deb_field(paragraph, "Size")?.parse()?,
    };
    validate_payload(version, payload, target).map(Some)
}

async fn check_linux_release(token: &str, target: Target) -> Result<Option<Release>> {
    let arch = command_output(
        Command::new("/usr/bin/dpkg").arg("--print-architecture"),
        "Не удалось определить архитектуру Debian/Ubuntu",
    )?;
    let arch = arch.trim();
    ensure!(
        matches!(arch, "amd64" | "arm64"),
        "Claude требует Ubuntu/Debian amd64 или arm64"
    );
    let client = client(token)?;
    let directory = tempfile::Builder::new()
        .prefix("wizard-claude-apt-")
        .tempdir()?;
    let key = directory.path().join("key.asc");
    let keyring = directory.path().join("keyring.gpg");
    let signed = directory.path().join("InRelease");
    let verified = directory.path().join("Release");
    fs::write(&key, apt_metadata(&client, "key.asc").await?)?;
    fs::write(
        &signed,
        apt_metadata(&client, "apt/stable/dists/stable/InRelease").await?,
    )?;
    command_output(
        Command::new("/usr/bin/gpg")
            .args(["--no-options", "--batch", "--homedir"])
            .arg(directory.path())
            .arg("--dearmor")
            .arg("--output")
            .arg(&keyring)
            .arg(&key),
        "Не удалось прочитать ключ APT; необходим gpg",
    )?;
    let status = command_output(
        Command::new("/usr/bin/gpgv")
            .arg("--homedir")
            .arg(directory.path())
            .arg("--keyring")
            .arg(&keyring)
            .args(["--status-fd", "1", "--output"])
            .arg(&verified)
            .arg(&signed),
        "Подпись репозитория APT не прошла проверку",
    )?;
    ensure!(
        status.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.len() > 2
                && fields[0] == "[GNUPG:]"
                && fields[1] == "VALIDSIG"
                && fields[2] == APT_SIGNER
        }),
        "Репозиторий APT подписан не ключом Anthropic"
    );
    let index = apt_metadata(
        &client,
        &format!("apt/stable/dists/stable/main/binary-{arch}/Packages"),
    )
    .await?;
    verify_apt_index(&fs::read_to_string(verified)?, arch, &index)?;
    parse_apt_release(core::str::from_utf8(&index)?, arch, target)
}

#[cfg(target_os = "linux")]
const LINUX_INSTALL: &str = r#"
set -eu
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
stage=$(mktemp -d /tmp/wizard-claude-install-XXXXXXXX) || exit 20
trap 'rm -rf -- "$stage"' EXIT
# Copy into root-owned storage before rechecking: the caller cannot replace this file.
cp -- "$1" "$stage/package.deb" || exit 20
actual=$(sha256sum "$stage/package.deb") || exit 20
[ "${actual%% *}" = "$3" ] || exit 20
[ "$(dpkg-deb -f "$stage/package.deb" Package)" = claude-desktop ] || exit 12
[ "$(dpkg-deb -f "$stage/package.deb" Version)" = "$2" ] || exit 13
[ "$(dpkg-deb -f "$stage/package.deb" Architecture)" = "$(dpkg --print-architecture)" ] || exit 14
for process in /proc/[0-9]*/exe; do
    executable=$(readlink "$process" 2>/dev/null || :)
    case "$executable" in /usr/lib/claude-desktop/*) exit 15 ;; esac
done
DEBIAN_FRONTEND=noninteractive apt-get --assume-yes --no-install-recommends --no-remove install "$stage/package.deb" || exit 16
[ "$(dpkg-query -W -f='${Status} ${Version}' claude-desktop)" = "install ok installed $2" ] || exit 17
"#;

#[cfg(target_os = "linux")]
fn install_linux(archive: &Path, release: &Release) -> Result<()> {
    let version = release.version.to_string();
    let metadata = command_output(
        Command::new("/usr/bin/dpkg-deb").arg("-f").arg(archive),
        "Не удалось прочитать пакет DEB",
    )?;
    ensure!(
        deb_field(&metadata, "Package")? == "claude-desktop"
            && deb_field(&metadata, "Version")? == version,
        "Неверный пакет или версия Claude"
    );
    let arch = command_output(
        Command::new("/usr/bin/dpkg").arg("--print-architecture"),
        "Не удалось определить архитектуру Ubuntu/Debian",
    )?;
    ensure!(
        matches!(arch.trim(), "amd64" | "arm64")
            && deb_field(&metadata, "Architecture")? == arch.trim(),
        "Архитектура DEB не соответствует системе"
    );
    let status = Command::new("/usr/bin/pkexec").args(["--disable-internal-agent", "/bin/sh", "-c", LINUX_INSTALL, "wizard-claude-install"])
        .arg(archive).arg(&version).arg(&release.sha256)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()
        .context("Не удалось запросить права администратора; необходим pkexec и графический агент Polkit")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "{}",
        match status.code() {
            Some(12..=14) => "DEB не соответствует выбранному пакету, версии или архитектуре",
            Some(15) => "Полностью закройте Claude Desktop и повторите установку",
            Some(17) => "APT завершил установку, но ожидаемая версия Claude не обнаружена",
            Some(20) => "Не удалось подготовить DEB или его SHA-256 не совпадает",
            Some(126 | 127) =>
                "Права администратора не получены. Проверьте запрос Polkit; пакет сохранён для повтора",
            _ =>
                "APT не удалось установить Claude. Проверьте доступ к репозиториям, зависимости и блокировку менеджера пакетов. Возможна частичная установка; пакет сохранён для повтора",
        }
    )
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

#[cfg(target_os = "macos")]
fn verify_bundle(path: &Path, version: &Version) -> Result<()> {
    let plist = plist::Value::from_file(path.join("Contents/Info.plist"))?;
    ensure!(
        plist
            .as_dictionary()
            .and_then(|p| p.get("CFBundleIdentifier"))
            .and_then(plist::Value::as_string)
            == Some(BUNDLE_ID),
        "Неверный идентификатор приложения"
    );

    let actual = read_app_version(path)?.context("В приложении отсутствует версия")?;
    ensure!(
        Version::parse(&actual)? == *version,
        "Версия приложения не совпадает с выбранной"
    );

    command_output(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "-R", SIGNING_REQUIREMENT])
            .arg(path),
        "Подпись Claude не прошла проверку Anthropic",
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

#[cfg(target_os = "windows")]
fn platform_feed(target: &Target) -> Result<(Url, String)> {
    #[derive(Deserialize)]
    struct SystemInfo {
        arch: String,
        os_version: String,
        has_msix: bool,
    }

    let output = command_output(
        &mut powershell(
            r"
$ErrorActionPreference = 'Stop'
@{
    arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    os_version = [Environment]::OSVersion.Version.ToString()
    has_msix = $null -ne (Get-AppxPackage -Name Claude)
} | ConvertTo-Json -Compress
",
        ),
        "Не удалось определить платформу Windows и пакет Claude",
    )?;
    let info: SystemInfo =
        serde_json::from_str(output.trim()).context("Некорректные сведения о платформе Windows")?;
    ensure!(
        matches!(info.arch.as_str(), "x64" | "arm64"),
        "Claude требует Windows x64 или ARM64"
    );
    ensure!(
        target.version.is_none() || info.has_msix,
        "Обнаружена старая установка Claude без MSIX. Удалите её самостоятельно перед установкой MSIX; Wizard не выполняет миграцию"
    );
    Ok((
        Url::parse(&format!(
            "https://releases.claude.com/api/desktop/win32/{}/msix/update",
            info.arch
        ))?,
        info.os_version,
    ))
}

#[cfg(target_os = "windows")]
const WINDOWS_PUBLISHER: &str = "CN=\"Anthropic, PBC\", O=\"Anthropic, PBC\", L=San Francisco, S=California, C=US, SERIALNUMBER=4860621, OID.2.5.4.15=Private Organization, OID.1.3.6.1.4.1.311.60.2.1.2=Delaware, OID.1.3.6.1.4.1.311.60.2.1.3=US";

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
fn windows_install_script(setup: &str, body: &str) -> String {
    format!(
        r#"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$phase = 1
function Write-InstallDiagnostic($failure) {{
    try {{
        # Do not format ErrorRecord, InvocationInfo, command arguments or environment.
        $message = $failure.Exception.Message.Replace("`r", '').Replace("`n", '\n')
        if ($message.Length -gt 4096) {{ $message = $message.Substring(0, 4096) }}
        $text = 'WIZARD_MSIX_ERROR: phase={{0}} HRESULT=0x{{1:X8}} {{2}}' -f $phase, $failure.Exception.HResult, $message
        if ($diagnosticPath) {{
            [IO.File]::WriteAllText($diagnosticPath, $text, [Text.UTF8Encoding]::new($false))
        }} else {{
            [Console]::Error.WriteLine($text)
        }}
    }} catch {{
        # Diagnostic failures must not replace the original exit code.
    }}
}}
try {{
    [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
    {setup}
    {body}
}} catch {{
    Write-InstallDiagnostic $_
    exit $phase
}}
"#
    )
}

#[cfg(target_os = "windows")]
const WINDOWS_INSTALL: &str = r"
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
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
    if ($identity.Name -cne 'Claude' -or $identity.Publisher -cne $package.publisher) { exit 12 }
    if ($identity.Version -cne $package.version) { exit 13 }
    $arch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    if ($arch -notin @('x64', 'arm64') -or $identity.ProcessorArchitecture -cne $arch) { exit 14 }

    $phase = 15
    if (Get-Process -Name Claude -ErrorAction SilentlyContinue) { exit 15 }
    if (!$install) { exit 0 }
    $phase = 16
    # No force-close, deferred registration, downgrade or remote dependencies.
    Add-AppxPackage -Path $package.archive | Out-Null
    $phase = 17
    $installed = Get-AppxPackage -Name Claude
    if ($null -eq $installed -or $installed.Publisher -cne $package.publisher -or $installed.Version.ToString() -cne $package.version) { exit 17 }
    exit 0
} catch {
    Write-InstallDiagnostic $_
    exit $phase
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
    $installed = Get-AppxPackage -Name Claude
    if ($null -eq $installed -or $installed.Publisher -cne $package.publisher -or $installed.Version.ToString() -cne $package.version) { exit 17 }
    exit 0
} catch {
    Write-InstallDiagnostic $_
    if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) { exit 19 }
    exit 21
}
"#;

#[cfg(target_os = "windows")]
fn install_windows(archive: &Path, version: &Version, sha256: &str) -> Result<()> {
    ensure!(
        version.pre.is_empty()
            && version.build.is_empty()
            && [version.major, version.minor, version.patch]
                .into_iter()
                .all(|part| u16::try_from(part).is_ok()),
        "Версия Claude не соответствует формату MSIX"
    );
    let diagnostics = if let Ok(directory) = tempfile::tempdir() {
        Some(directory)
    } else {
        log::warn!("Failed to prepare Claude MSIX installation diagnostic directory");
        None
    };
    let diagnostic_path = diagnostics
        .as_ref()
        .map(|directory| directory.path().join("install-error.txt"));
    let package = serde_json::json!({
        "diagnostic_path": diagnostic_path,
        "archive": archive,
        "publisher": WINDOWS_PUBLISHER,
        "version": format!("{version}.0"),
        "sha256": sha256,
        "sid": "",
    })
    .to_string();
    let setup = r"
$diagnosticPath = $null
$package = $env:WIZARD_CLAUDE_PACKAGE | ConvertFrom-Json
$package.sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
";
    // Reject corrupt packages and running Claude before asking for UAC.
    run_windows_install(
        powershell(&windows_install_script(
            &format!("{setup}\n$install = $false"),
            WINDOWS_INSTALL,
        ))
        .env("WIZARD_CLAUDE_PACKAGE", &package),
        None,
    )?;
    let elevated = windows_install_script(
        "$package = $packageJson | ConvertFrom-Json\n$install = $true",
        WINDOWS_INSTALL,
    );
    run_windows_install(
        powershell(&windows_install_script(
            setup,
            &format!("$body = @'\n{elevated}\n'@\n{WINDOWS_ELEVATE}"),
        ))
        .env("WIZARD_CLAUDE_PACKAGE", &package),
        diagnostic_path.as_deref(),
    )
}

#[cfg(target_os = "windows")]
fn run_windows_install(command: &mut Command, diagnostic_path: Option<&Path>) -> Result<()> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Не удалось запустить установку MSIX")?;
    if output.status.success() {
        return Ok(());
    }

    log::error!(
        "Claude MSIX installation failed: exit code {:?}",
        output.status.code()
    );
    for diagnostic in windows_install_diagnostics(&String::from_utf8_lossy(&output.stderr)) {
        log::error!("Claude MSIX installation: {diagnostic}");
    }
    if let Some(path) = diagnostic_path {
        match fs::File::open(path) {
            Ok(file) => {
                let mut text = String::new();
                if file.take(64 * 1024).read_to_string(&mut text).is_ok() {
                    for diagnostic in windows_install_diagnostics(&text) {
                        log::error!("Claude MSIX installation: {diagnostic}");
                    }
                } else {
                    log::warn!("Failed to read Claude MSIX installation diagnostic");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => log::warn!("Failed to open Claude MSIX installation diagnostic"),
        }
    }
    bail!(
        "{}",
        match output.status.code() {
            Some(10) => "Подпись MSIX не прошла проверку Anthropic",
            Some(11) => "Не удалось прочитать манифест MSIX",
            Some(12) => "Неверный идентификатор или издатель пакета Claude",
            Some(13) => "Версия MSIX не совпадает с выбранной",
            Some(14) => "Архитектура MSIX не соответствует Windows",
            Some(15) =>
                "Полностью закройте Claude Desktop, включая значок в трее, и повторите установку",
            Some(17) =>
                "Windows завершила установку, но ожидаемая версия Claude не обнаружена у исходного пользователя",
            Some(18) =>
                "В UAC выбрана другая учётная запись. Подтвердите повышение прав для того же пользователя, который запустил Wizard",
            Some(19) =>
                "Запрос прав администратора отменён. Пакет сохранён для повторной установки",
            Some(20) => "SHA-256 пакета MSIX не совпадает",
            Some(21) => "Не удалось запустить процесс установки с правами администратора",
            Some(22) => "Не удалось открыть и заблокировать пакет MSIX",
            Some(23) => "Не удалось вычислить SHA-256 пакета MSIX",
            _ =>
                "Windows не удалось установить MSIX. Проверьте зависимости пакета и ограничения системы. Пакет сохранён для повторной попытки",
        }
    )
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

pub fn install_release(download: &DownloadedRelease) -> Result<()> {
    let DownloadedRelease { directory, release } = download;
    let archive = directory.path().join(DOWNLOAD_NAME);
    ensure!(
        archive_sha256(&archive)? == release.sha256,
        "SHA-256 архива не совпадает"
    );

    #[cfg(target_os = "macos")]
    {
        // Keep the archive for retry, but clean extracted files after every attempt.
        let extracted = tempfile::Builder::new()
            .prefix("extracted-")
            .tempdir_in(directory.path())
            .context("Не удалось создать каталог распаковки")?;
        // bsdtar's default extraction rejects traversal through symlinks and '..'. Do not use -P.
        command_output(
            Command::new("/usr/bin/tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(extracted.path()),
            "Не удалось распаковать Claude",
        )?;
        let app = extracted.path().join("Claude.app");

        // Stage beside the destination so replacement uses rename on one filesystem.
        let parent = release
            .target
            .path
            .as_ref()
            .context("Не указан путь установки")?
            .parent()
            .context("Некорректный путь установки")?;
        let staging = tempfile::Builder::new()
            .prefix(".wizard-claude-")
            .tempdir_in(parent)
            .context("Нет прав на установку в каталог приложения")?;
        let staged = staging.path().join("Claude.app");
        command_output(
            Command::new("/usr/bin/ditto").arg(&app).arg(&staged),
            "Не удалось подготовить приложение к замене",
        )?;

        verify_bundle(&staged, &release.version)?;
        // Check all Claude instances, including a running app at a different path. Never terminate them.
        ensure!(
            !app_is_running(BUNDLE_ID),
            "Полностью закройте Claude Desktop, включая значок в меню, и повторите установку"
        );
        replace_bundle(staging, &staged, &release.target)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows(&archive, &release.version, &release.sha256)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux(&archive, release)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    bail!("Установка Claude Desktop на этой платформе ещё не реализована")
}

#[cfg(target_os = "macos")]
fn replace_bundle(staging: TempDir, staged: &Path, target: &Target) -> Result<()> {
    let destination = target.path.as_ref().context("Не указан путь установки")?;
    let previous = staging.path().join("previous.app");
    if target.version.is_some() {
        fs::rename(destination, &previous)
            .context("Не удалось переместить предыдущую версию Claude")?;
    }

    if let Err(error) = fs::rename(staged, destination) {
        if target.version.is_some() && fs::rename(&previous, destination).is_err() {
            let preserved = staging.keep();
            bail!(
                "Замена и восстановление Claude не удались. Предыдущая версия сохранена в {}",
                preserved.join("previous.app").display()
            );
        }
        return Err(error).context("Не удалось установить Claude; предыдущая версия сохранена");
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "bounded test fixtures")]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(version: Option<&str>) -> Target {
        Target {
            path: Some(PathBuf::from("Claude.app")),
            version: version.map(|v| Version::parse(v).unwrap()),
        }
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
        let mut release = parse_release(&serde_json::to_vec(&feed()).unwrap(), target(None))
            .unwrap()
            .unwrap();
        release.sha256 = archive_sha256(&archive).unwrap();
        // No personal application path is used, even if extraction unexpectedly succeeds.
        release.target.path = Some(directory.path().join("destination.app"));
        let download = DownloadedRelease { directory, release };

        for _ in 0..2 {
            let error = install_release(&download).unwrap_err();
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
        let failure = "throw [InvalidOperationException]::new('synthetic installation failure')";
        let run = |script: &str, path: &Path| {
            powershell(script)
                .env("WIZARD_TEST_DIAGNOSTIC", path)
                .stdin(Stdio::null())
                .output()
                .unwrap()
        };

        // A setup exception must be reported without dumping the script.
        let output = run(&windows_install_script(failure, ""), &path);
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8(output.stderr).unwrap();
        let diagnostic = windows_install_diagnostics(&stderr).next().unwrap();
        assert!(diagnostic.contains("HRESULT=0x80131509"));
        assert!(diagnostic.ends_with("synthetic installation failure"));
        assert!(!stderr.contains("throw [InvalidOperationException]"));

        // The same reporter writes the elevated channel; failure to write must
        // neither replace the original exit code nor leak raw stderr.
        let script = windows_install_script(
            "$diagnosticPath = $env:WIZARD_TEST_DIAGNOSTIC\n$phase = 16",
            failure,
        );
        let output = run(&script, &path);
        assert_eq!(output.status.code(), Some(16));
        assert!(output.stderr.is_empty());
        assert!(fs::read_to_string(&path).unwrap().contains("phase=16"));
        let output = run(&script, directory.path());
        assert_eq!(output.status.code(), Some(16));
        assert!(output.stderr.is_empty());

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
        let output = run(&script, &path);
        assert_eq!(output.status.code(), Some(16));
        assert!(output.stderr.is_empty());
        assert!(fs::read_to_string(&path).unwrap().contains("phase=16"));

        let cancellation =
            "function Start-Process { throw [ComponentModel.Win32Exception]::new(1223) }";
        let script = windows_install_script(
            setup,
            &format!("{cancellation}\n$body = ''\n{WINDOWS_ELEVATE}"),
        );
        let output = run(&script, &path);
        assert_eq!(output.status.code(), Some(19));
        assert!(
            windows_install_diagnostics(&String::from_utf8(output.stderr).unwrap())
                .next()
                .is_some()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn replacement_restores_old_bundle_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("Claude.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old"), "old").unwrap();
        let target = Target {
            path: Some(destination.clone()),
            version: Some(Version::new(1, 0, 0)),
        };

        let staging = tempfile::tempdir_in(root.path()).unwrap();
        let missing = staging.path().join("missing.app");

        assert!(replace_bundle(staging, &missing, &target).is_err());
        assert_eq!(fs::read_to_string(destination.join("old")).unwrap(), "old");
    }
}
