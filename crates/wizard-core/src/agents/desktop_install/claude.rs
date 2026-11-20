//! Claude Desktop: Anthropic's update feeds, APT repository and package identity.

#[cfg(target_os = "macos")]
use super::MacBundle;
use super::{AppSpec, DesktopApp, Release, Sha256Payload, Target, client, validate_payload};
#[cfg(target_os = "windows")]
use super::{WindowsPackage, command_output, powershell};
use crate::agents::{
    AgentDetection,
    detection::{self, DesktopAgent},
};
#[cfg(target_os = "macos")]
use crate::platform::macos::os_version;
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use reqwest::Url;
use semver::Version;
use serde::Deserialize;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

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
    update_to: Sha256Payload,
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
        return super::apt::check_release(token, target).await;
    }
    let url = feed_url(&target)?;
    let mut response = super::requests::feed_response(&client(token)?, url.as_str()).await?;
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
