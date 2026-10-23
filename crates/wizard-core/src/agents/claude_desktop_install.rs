//! macOS Claude installation. No settings writes, UI state, or persisted credentials.

use super::{
    AgentDetection,
    detection::{self, DesktopAgent},
};
use crate::platform::macos::{app_is_running, os_version, read_app_version};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use core::time::Duration;
use reqwest::{Client, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::TempDir;

const BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const FEED: &str = "https://api.anthropic.com/api/desktop/darwin/arm64/squirrel/update";
// Apple-issued Developer ID of Anthropic, not merely any valid code signature.
const SIGNING_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.anthropic.claudefordesktop\" and certificate leaf[subject.OU] = \"Q6L2SF6YDW\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";

#[derive(Clone)]
pub struct Target {
    pub path: PathBuf,
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
            let version = info
                .version
                .context("Не удалось определить установленную версию Claude")?;
            (
                info.path,
                Some(Version::parse(&version).context("Неизвестный формат версии Claude")?),
            )
        }
        AgentDetection::NotFound => (PathBuf::from(detection::CLAUDE_APP_PATH), None),
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
    ensure!(
        payload.version == feed.current_release,
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

    Ok(Some(Release {
        version,
        target,
        url,
        sha256: payload.sha256.to_ascii_lowercase(),
        size: payload.size,
    }))
}

pub async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
    let os_version = os_version();
    let mut url = Url::parse(FEED)?;
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
    let mut file = fs::File::create(directory.path().join("download.zip"))?;

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

fn command_output(command: &mut Command, message: &str) -> Result<String> {
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context(message.to_owned())?;
    ensure!(output.status.success(), "{message}");
    String::from_utf8(output.stdout).context(message.to_owned())
}

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
    let archive = directory.path().join("download.zip");
    ensure!(
        archive_sha256(&archive)? == release.sha256,
        "SHA-256 архива не совпадает"
    );

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

fn replace_bundle(staging: TempDir, staged: &Path, target: &Target) -> Result<()> {
    let previous = staging.path().join("previous.app");
    if target.version.is_some() {
        fs::rename(&target.path, &previous)
            .context("Не удалось переместить предыдущую версию Claude")?;
    }

    if let Err(error) = fs::rename(staged, &target.path) {
        if target.version.is_some() && fs::rename(&previous, &target.path).is_err() {
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
            path: detection::CLAUDE_APP_PATH.into(),
            version: version.map(|v| Version::parse(v).unwrap()),
        }
    }

    fn feed() -> serde_json::Value {
        json!({"currentRelease":"3.2.1", "releases":[{"version":"3.2.1", "updateTo":{
            "version":"3.2.1", "url":"https://downloads.claude.ai/releases/darwin/arm64/3.2.1/Claude.zip",
            "sha256":"a".repeat(64), "size":1024
        }}]})
    }

    #[test]
    fn feed_checks_version_and_origin() {
        let value = feed();
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(parse_release(&bytes, target(None)).unwrap().is_some());
        assert!(
            parse_release(&bytes, target(Some("3.2.0")))
                .unwrap()
                .is_some()
        );
        assert!(
            parse_release(&bytes, target(Some("3.2.1")))
                .unwrap()
                .is_none()
        );
        assert!(
            parse_release(&bytes, target(Some("3.2.2")))
                .unwrap()
                .is_none()
        );

        for url in [
            "http://downloads.claude.ai/releases/darwin/arm64/3.2.1/Claude.zip",
            "https://evil.test/Claude.zip",
        ] {
            let mut invalid = feed();
            invalid["releases"][0]["updateTo"]["url"] = json!(url);
            assert!(parse_release(&serde_json::to_vec(&invalid).unwrap(), target(None)).is_err());
        }

        let mut relocated = feed();
        relocated["releases"][0]["updateTo"]["url"] =
            json!("https://downloads.claude.ai/new-location/archive?download=1");
        assert!(
            parse_release(&serde_json::to_vec(&relocated).unwrap(), target(None))
                .unwrap()
                .is_some()
        );

        for (key, value) in [
            ("sha256", json!("bad")),
            ("size", json!(MAX_DOWNLOAD + 1)),
            ("version", json!("1.0.0")),
        ] {
            let mut invalid = feed();
            invalid["releases"][0]["updateTo"][key] = value;
            assert!(parse_release(&serde_json::to_vec(&invalid).unwrap(), target(None)).is_err());
        }
    }

    #[test]
    fn failed_install_keeps_archive_for_retry_and_cleans_extraction() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("download.zip");
        fs::write(&archive, b"invalid zip").unwrap();
        let mut release = parse_release(&serde_json::to_vec(&feed()).unwrap(), target(None))
            .unwrap()
            .unwrap();
        release.sha256 = archive_sha256(&archive).unwrap();
        // No personal application path is used, even if extraction unexpectedly succeeds.
        release.target.path = directory.path().join("destination.app");
        let download = DownloadedRelease { directory, release };

        for _ in 0..2 {
            let error = install_release(&download).unwrap_err();
            assert_eq!(error.to_string(), "Не удалось распаковать Claude");
            assert_eq!(fs::read(&archive).unwrap(), b"invalid zip");
            assert_eq!(fs::read_dir(download.directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn replacement_restores_old_bundle_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("Claude.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old"), "old").unwrap();
        let target = Target {
            path: destination.clone(),
            version: Some(Version::new(1, 0, 0)),
        };

        let staging = tempfile::tempdir_in(root.path()).unwrap();
        let missing = staging.path().join("missing.app");

        assert!(replace_bundle(staging, &missing, &target).is_err());
        assert_eq!(fs::read_to_string(destination.join("old")).unwrap(), "old");
    }
}
