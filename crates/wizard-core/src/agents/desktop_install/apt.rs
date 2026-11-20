//! Signed APT feeds and DEB installation for Linux desktop apps.
use super::{DesktopApp, Release, Sha256Payload, Target, client, command_output, validate_payload};
#[cfg(target_os = "linux")]
use anyhow::bail;
use anyhow::{Context as _, Result, anyhow, ensure};
use core::time::Duration;
use reqwest::Client;
use semver::Version;
use sha2::{Digest as _, Sha256};
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::{fs, path::Path, process::Command};

const CLAUDE_APT_ORIGIN: &str = "https://downloads.claude.ai/claude-desktop";
const CLAUDE_APT_SIGNER: &str = "31DDDE24DDFAB679F42D7BD2BAA929FF1A7ECACE";
const CHATGPT_APT_SIGNER: &str = "3BFA0E4AE8B8CC16A2D9BA684A3B4A566C4660E4";

async fn apt_metadata(client: &Client, url: String) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
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

fn verify_apt_index(release: &str, arch: &str, index: &[u8], app: DesktopApp) -> Result<()> {
    // OpenAI publishes Date but not Valid-Until; keep expiry mandatory for Anthropic.
    if app == DesktopApp::Claude || release.lines().any(|line| line.starts_with("Valid-Until:")) {
        let valid_until = deb_field(release, "Valid-Until")?.replace(" UTC", " +0000");
        let expiry = chrono::DateTime::parse_from_rfc2822(&valid_until)
            .context("Некорректный срок действия метаданных APT")?;
        ensure!(
            expiry > chrono::Utc::now(),
            "Подписанные метаданные APT устарели"
        );
    }
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

fn deb_package(app: DesktopApp) -> &'static str {
    match app {
        DesktopApp::Claude => "claude-desktop",
        DesktopApp::ChatGpt => "chatgpt",
    }
}

fn apt_base(app: DesktopApp) -> &'static str {
    match app {
        DesktopApp::Claude => "https://downloads.claude.ai/claude-desktop/apt/stable",
        DesktopApp::ChatGpt => "https://persistent.oaistatic.com/codex-app-prod/linux/deb",
    }
}

fn parse_apt_release(index: &str, arch: &str, target: Target) -> Result<Option<Release>> {
    let mut latest: Option<(Version, &str)> = None;
    for paragraph in index.split("\n\n") {
        if deb_field(paragraph, "Package").ok() != Some(deb_package(target.app))
            || deb_field(paragraph, "Architecture").ok() != Some(arch)
        {
            continue;
        }
        let version = Version::parse(deb_field(paragraph, "Version")?)
            .context("Некорректная версия приложения в APT")?;
        if latest
            .as_ref()
            .is_none_or(|(current, _)| version > *current)
        {
            latest = Some((version, paragraph));
        }
    }
    let (version, paragraph) = latest.context("В APT нет приложения для этой архитектуры")?;
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
    let payload = Sha256Payload {
        version: version.to_string(),
        url: format!("{}/{filename}", apt_base(target.app)),
        sha256: deb_field(paragraph, "SHA256")?.to_owned(),
        size: deb_field(paragraph, "Size")?.parse()?,
    };
    validate_payload(version, payload, target).map(Some)
}

pub(super) async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
    let arch = command_output(
        Command::new("/usr/bin/dpkg").arg("--print-architecture"),
        "Не удалось определить архитектуру Debian/Ubuntu",
    )?;
    let arch = arch.trim();
    ensure!(
        matches!(arch, "amd64" | "arm64"),
        "Требуется Ubuntu/Debian amd64 или arm64"
    );
    let client = client(token)?;
    let directory = tempfile::Builder::new()
        .prefix("wizard-desktop-apt-")
        .tempdir()?;
    let key = directory.path().join("key.asc");
    let keyring = directory.path().join("keyring.gpg");
    let signed = directory.path().join("InRelease");
    let verified = directory.path().join("Release");
    let app = target.app;
    let base = apt_base(app);
    let fingerprint = match app {
        DesktopApp::Claude => CLAUDE_APT_SIGNER,
        DesktopApp::ChatGpt => CHATGPT_APT_SIGNER,
    };
    fs::write(
        &signed,
        apt_metadata(&client, format!("{base}/dists/stable/InRelease")).await?,
    )?;
    if app == DesktopApp::ChatGpt {
        fs::write(&keyring, include_bytes!("chatgpt-archive-keyring.gpg"))?;
    } else {
        fs::write(
            &key,
            apt_metadata(&client, format!("{CLAUDE_APT_ORIGIN}/key.asc")).await?,
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
    }
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
                && fields[2] == fingerprint
        }),
        "Репозиторий APT подписан не ожидаемым ключом"
    );
    let index = apt_metadata(
        &client,
        format!("{base}/dists/stable/main/binary-{arch}/Packages"),
    )
    .await?;
    verify_apt_index(&fs::read_to_string(verified)?, arch, &index, app)?;
    parse_apt_release(core::str::from_utf8(&index)?, arch, target)
}

#[cfg(target_os = "linux")]
const LINUX_INSTALL: &str = r#"
set -eu
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 077
stage=$(mktemp -d /tmp/wizard-desktop-install-XXXXXXXX) || exit 20
trap 'rm -rf -- "$stage"' EXIT
# Copy stdin into root-owned storage before rechecking: the caller cannot replace this file, and
# root may be unable to read the caller's cache (NFS root_squash, FUSE).
cat > "$stage/package.deb" || exit 20
actual=$(sha256sum "$stage/package.deb") || exit 20
[ "${actual%% *}" = "$2" ] || exit 20
case "$4" in chatgpt|claude-desktop) ;; *) exit 12 ;; esac
[ "$(dpkg-deb -f "$stage/package.deb" Package)" = "$4" ] || exit 12
[ "$(dpkg-deb -f "$stage/package.deb" Version)" = "$1" ] || exit 13
[ "$(dpkg-deb -f "$stage/package.deb" Architecture)" = "$(dpkg --print-architecture)" ] || exit 14
claude_processes() {
    for process in /proc/[0-9]*; do
        case "$(readlink "$process/exe" 2>/dev/null || :)" in /usr/lib/claude-desktop/*) echo "${process#/proc/}" ;; esac
    done
}
if [ "$4" = chatgpt ]; then
    # The unprivileged caller handles graceful shutdown; never signal from root.
    for process in /proc/[0-9]*; do
        case "$(readlink "$process/exe" 2>/dev/null || :)" in /usr/lib/chatgpt/*) exit 15 ;; esac
    done
else
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
fi
DEBIAN_FRONTEND=noninteractive apt-get --assume-yes --no-install-recommends --no-remove install "$stage/package.deb" || exit 16
[ "$(dpkg-query -W -f='${Status} ${Version}' "$4")" = "install ok installed $1" ] || exit 17
"#;

#[cfg(target_os = "linux")]
/// For Claude, `close` permits shutdown after Polkit approval. ChatGPT is closed by the caller.
pub(super) fn install(archive: &Path, release: &Release, sha256: &str, close: bool) -> Result<()> {
    let product = release.target.app.product();
    let version = release.version.to_string();
    let metadata = command_output(
        Command::new("/usr/bin/dpkg-deb").arg("-f").arg(archive),
        "Не удалось прочитать пакет DEB",
    )?;
    ensure!(
        deb_field(&metadata, "Package")? == deb_package(release.target.app)
            && deb_field(&metadata, "Version")? == version,
        "Неверный пакет или версия {product}"
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
    let status = Command::new("/usr/bin/pkexec").args(["--disable-internal-agent", "/bin/sh", "-c", LINUX_INSTALL, "wizard-desktop-install"])
        .arg(&version).arg(sha256).arg(if close { "close" } else { "keep" }).arg(deb_package(release.target.app))
        .stdin(fs::File::open(archive).context("Не удалось открыть пакет DEB")?)
        .stdout(Stdio::null()).stderr(Stdio::null()).status()
        .context("Не удалось запросить права администратора; необходим pkexec и графический агент Polkit")?;
    if status.success() {
        return Ok(());
    }
    match status.code() {
        Some(12..=14) => bail!("DEB не соответствует выбранному пакету, версии или архитектуре"),
        Some(15) => bail!("Полностью закройте {product} и повторите установку"),
        Some(25) => bail!(
            "{product} запущен у другого пользователя. Попросите его закрыть {product} и повторите установку"
        ),
        Some(26) => bail!(
            "{product} не закрылся. Закройте его вручную, включая значок в трее, и повторите установку"
        ),
        Some(17) => bail!("APT завершил установку, но ожидаемая версия {product} не обнаружена"),
        Some(20) => bail!("Не удалось подготовить DEB или его SHA-256 не совпадает"),
        Some(126 | 127) => bail!(
            "Права администратора не получены. Проверьте запрос Polkit; пакет сохранён для повтора"
        ),
        _ => bail!(
            "APT не удалось установить {product}. Проверьте доступ к репозиториям, зависимости и блокировку менеджера пакетов. Возможна частичная установка; пакет сохранён для повтора"
        ),
    }
}
