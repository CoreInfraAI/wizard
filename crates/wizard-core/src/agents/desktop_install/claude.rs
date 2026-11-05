//! Claude Desktop: Anthropic's update feeds, APT repository and package identity.

#[cfg(target_os = "macos")]
use super::MacBundle;
use super::{
    AppSpec, DesktopApp, Integrity, MAX_DOWNLOAD, Release, Target, client, command_output,
};
#[cfg(target_os = "windows")]
use super::{WindowsPackage, powershell};
use crate::agents::{
    AgentDetection,
    detection::{self, DesktopAgent},
};
#[cfg(target_os = "macos")]
use crate::platform::macos::os_version;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::time::Duration;
use reqwest::{Client, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::{fs, path::Path, process::Command};

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.anthropic.claudefordesktop";

#[cfg(target_os = "macos")]
const FEED: &str = if cfg!(target_arch = "aarch64") {
    "https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update"
} else {
    "https://api.anthropic.com/api/desktop/darwin/x64/squirrel/update"
};
// Apple-issued Developer ID of Anthropic, not merely any valid code signature.
#[cfg(target_os = "macos")]
const SIGNING_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.anthropic.claudefordesktop\" and certificate leaf[subject.OU] = \"Q6L2SF6YDW\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";

pub(super) const SPEC: AppSpec = AppSpec {
    name: "Claude",
    product: "Claude Desktop",
    #[cfg(target_os = "macos")]
    bundle: MacBundle {
        file_name: "Claude.app",
        id: BUNDLE_ID,
        requirement: SIGNING_REQUIREMENT,
        vendor: "Anthropic",
    },
    #[cfg(target_os = "windows")]
    package: WindowsPackage {
        name: "Claude",
        process: "Claude",
        // Claude Code also runs claude.exe.
        process_in_package: true,
        publisher: WINDOWS_PUBLISHER,
        signer: "Anthropic",
    },
};

pub(super) fn unsupported_reason() -> Option<String> {
    (!cfg!(any(
        target_os = "macos",
        target_os = "windows",
        target_os = "linux"
    )))
    .then(|| "Установка Claude Desktop на этой платформе не поддерживается".to_owned())
}

pub(super) fn detect_target(last_path: Option<&Path>) -> Result<Target> {
    let (path, version) = match detection::detect_desktop(DesktopAgent::Claude, last_path) {
        AgentDetection::Found(info) => {
            let version = info
                .version
                .context("Не удалось определить установленную версию Claude")?;

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

    Ok(Target::new(DesktopApp::Claude, path, version))
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

pub(super) fn parse_release(bytes: &[u8], target: Target) -> Result<Option<Release>> {
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
        display_version: version.to_string(),
        version,
        target,
        url,
        integrity: Integrity::Sha256(payload.sha256),
        size: Some(payload.size),
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

pub(super) async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
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
# Copy stdin into root-owned storage before rechecking: the caller cannot replace this file, and
# root may be unable to read the caller's cache (NFS root_squash, FUSE).
cat > "$stage/package.deb" || exit 20
actual=$(sha256sum "$stage/package.deb") || exit 20
[ "${actual%% *}" = "$2" ] || exit 20
[ "$(dpkg-deb -f "$stage/package.deb" Package)" = claude-desktop ] || exit 12
[ "$(dpkg-deb -f "$stage/package.deb" Version)" = "$1" ] || exit 13
[ "$(dpkg-deb -f "$stage/package.deb" Architecture)" = "$(dpkg --print-architecture)" ] || exit 14
claude_processes() {
    for process in /proc/[0-9]*; do
        case "$(readlink "$process/exe" 2>/dev/null || :)" in /usr/lib/claude-desktop/*) echo "${process#/proc/}" ;; esac
    done
}
running=$(claude_processes)
if [ -n "$running" ]; then
    # Close only after Polkit approval, and only the requesting user's application.
    [ -n "${PKEXEC_UID:-}" ] || exit 15
    for pid in $running; do
        # The real UID: Chromium's sandboxed processes make /proc/<pid> root-owned.
        owner=$(awk '/^Uid:/ { print $2; exit }' "/proc/$pid/status" 2>/dev/null) || continue
        [ -z "$owner" ] || [ "$owner" = "$PKEXEC_UID" ] || exit 25
    done
    [ "$3" = close ] || exit 15
    # Electron quits cleanly on SIGTERM to its main process; helper processes have --type=.
    for pid in $running; do
        tr '\0' '\n' 2>/dev/null < "/proc/$pid/cmdline" | grep -q -e '--type=' || kill -TERM "$pid" 2>/dev/null || :
    done
    attempts=0
    while [ -n "$(claude_processes)" ]; do
        [ "$attempts" -lt 60 ] || exit 26
        attempts=$((attempts + 1))
        sleep 0.5
    done
fi
DEBIAN_FRONTEND=noninteractive apt-get --assume-yes --no-install-recommends --no-remove install "$stage/package.deb" || exit 16
[ "$(dpkg-query -W -f='${Status} ${Version}' claude-desktop)" = "install ok installed $1" ] || exit 17
"#;

#[cfg(target_os = "linux")]
/// `close`: ask the user's running Claude Desktop to quit after Polkit approval.
pub(super) fn install_linux(
    archive: &Path,
    release: &Release,
    sha256: &str,
    close: bool,
) -> Result<()> {
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
        .arg(&version).arg(sha256).arg(if close { "close" } else { "keep" })
        .stdin(fs::File::open(archive).context("Не удалось открыть пакет DEB")?)
        .stdout(Stdio::null()).stderr(Stdio::null()).status()
        .context("Не удалось запросить права администратора; необходим pkexec и графический агент Polkit")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "{}",
        match status.code() {
            Some(12..=14) => "DEB не соответствует выбранному пакету, версии или архитектуре",
            Some(15) => "Полностью закройте Claude Desktop и повторите установку",
            Some(25) =>
                "Claude Desktop запущен у другого пользователя. Попросите его закрыть приложение и повторите установку",
            Some(26) =>
                "Claude Desktop не закрылся. Закройте его вручную, включая значок в трее, и повторите установку",
            Some(17) => "APT завершил установку, но ожидаемая версия Claude не обнаружена",
            Some(20) => "Не удалось подготовить DEB или его SHA-256 не совпадает",
            Some(126 | 127) =>
                "Права администратора не получены. Проверьте запрос Polkit; пакет сохранён для повтора",
            _ =>
                "APT не удалось установить Claude. Проверьте доступ к репозиториям, зависимости и блокировку менеджера пакетов. Возможна частичная установка; пакет сохранён для повтора",
        }
    )
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
