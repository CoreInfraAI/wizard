//! `ChatGPT` with `ChatGPT` Work and Codex (`com.openai.codex`, formerly the Codex app):
//! `OpenAI`'s Sparkle appcast, Windows release feed and installation layout.
//! `ChatGPT` Classic (`com.openai.chat`) is a different application.

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use super::DesktopApp;
#[cfg(target_os = "windows")]
use super::command_output;
use super::{AppSpec, Integrity, MAX_DOWNLOAD, Release, Target, client, download_url, fetch_feed};
#[cfg(target_os = "macos")]
use super::{MacBundle, Rename};
#[cfg(target_os = "windows")]
use super::{WindowsPackage, powershell};
#[cfg(target_os = "macos")]
use crate::platform::macos::{AppInfo, os_version, read_app_info};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use semver::Version;
use serde::Deserialize;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.openai.codex";
#[cfg(target_os = "macos")]
const CLASSIC_BUNDLE_ID: &str = "com.openai.chat";
/// Apple Silicon builds; Intel builds have their own appcast.
const APPCAST_ARM64: &str = "https://persistent.oaistatic.com/codex-app-prod/appcast.xml";
const APPCAST_X64: &str = "https://persistent.oaistatic.com/codex-app-prod/appcast-x64.xml";
const WINDOWS_RELEASE: &str =
    "https://persistent.oaistatic.com/codex-app-prod/windows-release.json";
const WINDOWS_STORE_UPDATE: &str =
    "https://persistent.oaistatic.com/codex-app-prod/windows-store-update.json";
const DOWNLOAD_HOST: &str = "persistent.oaistatic.com";
/// `SUPublicEDKey` of `ChatGPT`: its own Sparkle updater accepts only archives signed with it.
const SPARKLE_PUBLIC_KEY: &str = "mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k=";
const WINDOWS_PACKAGE: &str = "OpenAI.Codex";
/// The Microsoft Store signs the package; `OpenAI` publishes the same file directly.
#[cfg(target_os = "windows")]
const WINDOWS_PUBLISHER: &str = "CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B";
const UNSUPPORTED: &str = "ChatGPT не выпускается для этой системы";

pub(super) const SPEC: AppSpec = AppSpec {
    name: "ChatGPT",
    product: "ChatGPT",
    #[cfg(target_os = "macos")]
    bundle: MacBundle {
        file_name: "ChatGPT.app",
        id: BUNDLE_ID,
        requirement: "=anchor apple generic and identifier \"com.openai.codex\" and certificate leaf[subject.OU] = \"2DC432GLL2\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
        vendor: "OpenAI",
    },
    #[cfg(target_os = "windows")]
    package: WindowsPackage {
        name: WINDOWS_PACKAGE,
        process: "ChatGPT",
        // ChatGPT Classic for Windows also runs ChatGPT.exe.
        process_in_package: true,
        publisher: WINDOWS_PUBLISHER,
        signer: "Microsoft Store",
    },
};

pub(super) fn unsupported_reason() -> Option<String> {
    (!cfg!(any(target_os = "macos", target_os = "windows"))).then(|| UNSUPPORTED.to_owned())
}

/// Whether this Mac has Apple Silicon, also when an x64 Wizard runs under Rosetta.
fn apple_silicon() -> bool {
    cfg!(target_os = "macos")
        && (cfg!(target_arch = "aarch64")
            || Command::new("/usr/sbin/sysctl")
                .args(["-n", "hw.optional.arm64"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"1"))
}

#[cfg(target_os = "macos")]
pub(super) fn detect_target() -> Result<Target> {
    let mut directories = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join("Applications"));
    }
    detect_in(&directories)
}

/// Updates `ChatGPT.app` where it is. Otherwise installs next to an older `Codex.app`, which it
/// supersedes, or into the first directory, moving `ChatGPT` Classic aside after confirmation.
#[cfg(target_os = "macos")]
fn detect_in(directories: &[PathBuf]) -> Result<Target> {
    let mut current = None;
    let mut superseded = Vec::new();
    let mut superseded_version: Option<Version> = None;
    for directory in directories {
        let path = directory.join(SPEC.bundle.file_name);
        if current.is_none()
            && let Some(info) = bundle_info(&path)?
            && info.identifier.as_deref() == Some(BUNDLE_ID)
        {
            current = Some((path, bundle_version(&info)?));
        }
        // Copies installed before OpenAI renamed the application.
        let legacy = directory.join("Codex.app");
        if let Some(info) = bundle_info(&legacy)?
            && info.identifier.as_deref() == Some(BUNDLE_ID)
        {
            let version = bundle_version(&info)?;
            if superseded_version
                .as_ref()
                .is_none_or(|newest| version > *newest)
            {
                superseded_version = Some(version);
            }
            superseded.push(legacy);
        }
    }

    if let Some((path, version)) = current {
        return Ok(Target {
            superseded,
            ..Target::new(DesktopApp::ChatGpt, Some(path), Some(version))
        });
    }

    let directory = superseded
        .first()
        .and_then(|path| path.parent())
        .or_else(|| directories.first().map(PathBuf::as_path))
        .context("Не указан каталог приложений")?
        .to_owned();
    let path = directory.join(SPEC.bundle.file_name);
    let rename = match bundle_info(&path)? {
        None => None,
        Some(info) if info.identifier.as_deref() == Some(CLASSIC_BUNDLE_ID) => {
            let to = directory.join("ChatGPT Classic.app");
            ensure!(
                bundle_info(&to)?.is_none(),
                "ChatGPT Classic занимает {}, а {} уже существует. Переименуйте или удалите одно из приложений и повторите проверку",
                path.display(),
                to.display()
            );
            Some(Rename {
                from: path.clone(),
                to,
                bundle_id: CLASSIC_BUNDLE_ID,
                name: "ChatGPT Classic",
            })
        }
        Some(_) => bail!(
            "{} занят другим приложением. Переименуйте или удалите его и повторите проверку",
            path.display()
        ),
    };
    Ok(Target {
        app: DesktopApp::ChatGpt,
        path: Some(path),
        version: superseded_version,
        replaces: false,
        rename,
        superseded,
    })
}

/// Identity of whatever occupies `path`; an unreadable bundle is some other application.
#[cfg(target_os = "macos")]
fn bundle_info(path: &Path) -> Result<Option<AppInfo>> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Не удалось проверить {}", path.display()));
        }
    }
    Ok(Some(read_app_info(path).unwrap_or(AppInfo {
        identifier: None,
        version: None,
    })))
}

#[cfg(target_os = "macos")]
fn bundle_version(info: &AppInfo) -> Result<Version> {
    Version::parse(
        info.version
            .as_deref()
            .context("Не удалось определить установленную версию ChatGPT")?,
    )
    .context("Неизвестный формат версии ChatGPT")
}

#[cfg(target_os = "windows")]
pub(super) fn detect_target() -> Result<Target> {
    #[derive(Deserialize)]
    struct Installed {
        version: String,
        publisher: String,
    }

    let output = command_output(
        &mut powershell(
            r"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$package = Get-AppxPackage -Name 'OpenAI.Codex' | Sort-Object Version -Descending | Select-Object -First 1
if ($null -eq $package) { 'null' } else {
    @{ version = $package.Version.ToString(); publisher = $package.Publisher } | ConvertTo-Json -Compress
}
",
        ),
        "Не удалось определить установленный пакет ChatGPT",
    )?;
    let installed: Option<Installed> =
        serde_json::from_str(output.trim()).context("Некорректные сведения о пакете ChatGPT")?;
    let version = match installed {
        None => None,
        Some(installed) => {
            ensure!(
                installed.publisher == WINDOWS_PUBLISHER,
                "Установлен пакет ChatGPT другого издателя; Wizard не обновляет его"
            );
            Some(package_version(&installed.version)?)
        }
    };
    Ok(Target::new(DesktopApp::ChatGpt, None, version))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn detect_target() -> Result<Target> {
    bail!(UNSUPPORTED)
}

pub(super) async fn check_release(token: &str, target: Target) -> Result<Option<Release>> {
    let client = client(token)?;
    if cfg!(target_os = "windows") {
        let architecture = windows_architecture()?;
        let bytes = fetch_feed(&client, WINDOWS_RELEASE, 64 * 1024).await?;
        let Some(mut release) = parse_windows_release(&bytes, &architecture, target)? else {
            return Ok(None);
        };
        // ChatGPT reports its application version, which differs from the package version.
        match fetch_feed(&client, WINDOWS_STORE_UPDATE, 64 * 1024)
            .await
            .and_then(|bytes| application_version(&bytes, &release.version))
        {
            Ok(Some(version)) => release.display_version = version,
            Ok(None) => log::warn!(
                "ChatGPT Store update manifest does not describe package {}",
                release.version
            ),
            Err(error) => log::warn!("ChatGPT Store update manifest unavailable: {error:#}"),
        }
        Ok(Some(release))
    } else {
        // Install the native build, not the one matching Wizard's own architecture.
        let (feed, architecture) = if apple_silicon() {
            (APPCAST_ARM64, "arm64")
        } else {
            (APPCAST_X64, "x86_64")
        };
        let system = macos_version()?;
        let bytes = fetch_feed(&client, feed, 2 * 1024 * 1024).await?;
        let text = core::str::from_utf8(&bytes).context("Некорректный ответ сервера обновлений")?;
        parse_appcast(text, &system, architecture, target)
    }
}

#[cfg(target_os = "macos")]
fn macos_version() -> Result<Version> {
    lenient_version(&os_version()).context("Не удалось определить версию macOS")
}

#[cfg(not(target_os = "macos"))]
fn macos_version() -> Result<Version> {
    bail!(UNSUPPORTED)
}

#[cfg(target_os = "windows")]
fn windows_architecture() -> Result<String> {
    let output = command_output(
        &mut powershell(
            "[Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()",
        ),
        "Не удалось определить архитектуру Windows",
    )?;
    let architecture = output.trim();
    ensure!(
        matches!(architecture, "x64" | "arm64"),
        "ChatGPT требует Windows x64 или ARM64"
    );
    Ok(architecture.to_owned())
}

#[cfg(not(target_os = "windows"))]
fn windows_architecture() -> Result<String> {
    bail!(UNSUPPORTED)
}

/// Parses `13.0` and `15.5.1` alike.
fn lenient_version(text: &str) -> Option<Version> {
    let mut parts = text.trim().split('.');
    let mut next = || parts.next().map_or(Some(0), |part| part.parse().ok());
    let version = Version::new(next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// Windows package versions have four parts; the Store requires the last one to be zero.
fn package_version(text: &str) -> Result<Version> {
    let parts: Vec<u64> = text
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()
        .filter(|parts: &Vec<u64>| parts.len() == 4 && parts[3] == 0)
        .context("Неизвестный формат версии пакета ChatGPT")?;
    Ok(Version::new(parts[0], parts[1], parts[2]))
}

#[derive(Default)]
struct AppcastItem {
    version: Option<String>,
    minimum_system: Option<String>,
    hardware: Option<String>,
    /// Items on a named channel, such as beta, are not for everyone.
    channel: bool,
    enclosure: Option<Enclosure>,
}

struct Enclosure {
    url: String,
    length: String,
    signature: String,
}

#[derive(Clone, Copy)]
enum AppcastField {
    Version,
    MinimumSystem,
    Hardware,
}

fn appcast_items(text: &str) -> Result<Vec<AppcastItem>> {
    use quick_xml::{Reader, events::Event};

    let invalid = || anyhow!("Некорректный ответ сервера обновлений");
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut item: Option<AppcastItem> = None;
    // Deltas update only a specific older build; Wizard installs full archives.
    let mut deltas = 0_usize;
    let mut field = None;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(element) => {
                let name = element.local_name();
                match (name.as_ref(), &mut item) {
                    ("item", _) => item = Some(AppcastItem::default()),
                    ("deltas", Some(_)) => deltas += 1,
                    ("shortVersionString", Some(_)) => field = Some(AppcastField::Version),
                    ("minimumSystemVersion", Some(_)) => {
                        field = Some(AppcastField::MinimumSystem);
                    }
                    ("hardwareRequirements", Some(_)) => field = Some(AppcastField::Hardware),
                    // Inside an item, only Sparkle's channel element has this name.
                    ("channel", Some(item)) => item.channel = true,
                    ("enclosure", Some(item)) if deltas == 0 => {
                        item.enclosure = enclosure(&element)?;
                    }
                    _ => {}
                }
            }
            Event::Empty(element) => {
                if let ("enclosure", Some(item)) = (element.local_name().as_ref(), &mut item)
                    && deltas == 0
                {
                    item.enclosure = enclosure(&element)?;
                }
            }
            Event::Text(text) => {
                if let (Some(field), Some(item)) = (field, &mut item) {
                    let value = Some(text.xml10_content().trim().to_owned());
                    match field {
                        AppcastField::Version => item.version = value,
                        AppcastField::MinimumSystem => item.minimum_system = value,
                        AppcastField::Hardware => item.hardware = value,
                    }
                }
            }
            Event::End(element) => {
                match element.local_name().as_ref() {
                    "item" => items.extend(item.take()),
                    "deltas" => deltas = deltas.saturating_sub(1),
                    _ => {}
                }
                field = None;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(items)
}

/// An enclosure without a URL, size or `EdDSA` signature cannot be installed.
fn enclosure(element: &quick_xml::events::BytesStart<'_>) -> Result<Option<Enclosure>> {
    let (mut url, mut length, mut signature) = (None, None, None);
    for attribute in element.attributes() {
        let attribute = attribute.context("Некорректный ответ сервера обновлений")?;
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .context("Некорректный ответ сервера обновлений")?
            .into_owned();
        match attribute.key.local_name().as_ref() {
            "url" => url = Some(value),
            "length" => length = Some(value),
            "edSignature" => signature = Some(value),
            _ => {}
        }
    }
    Ok(url
        .zip(length)
        .zip(signature)
        .map(|((url, length), signature)| Enclosure {
            url,
            length,
            signature,
        }))
}

/// Selects the newest full archive for this macOS from the default channel. Items that require
/// other hardware than `architecture`, as Sparkle names it, do not run on this Mac.
fn parse_appcast(
    text: &str,
    system: &Version,
    architecture: &str,
    target: Target,
) -> Result<Option<Release>> {
    let mut newest: Option<(Version, Enclosure)> = None;
    for item in appcast_items(text)? {
        if item.channel
            || item.hardware.as_deref().is_some_and(|hardware| {
                !hardware
                    .split([',', ' '])
                    .any(|required| required == architecture)
            })
            || item.minimum_system.as_deref().is_some_and(|minimum| {
                lenient_version(minimum).is_none_or(|minimum| minimum > *system)
            })
        {
            continue;
        }
        let (Some(version), Some(enclosure)) = (
            item.version
                .as_deref()
                .and_then(|version| Version::parse(version).ok()),
            item.enclosure,
        ) else {
            continue;
        };
        if newest
            .as_ref()
            .is_none_or(|(current, _)| version > *current)
        {
            newest = Some((version, enclosure));
        }
    }
    let (version, enclosure) = newest.context("В ленте обновлений нет ChatGPT для этого Mac")?;
    if target
        .version
        .as_ref()
        .is_some_and(|current| version <= *current)
    {
        return Ok(None);
    }

    let url = download_url(&enclosure.url, DOWNLOAD_HOST, "/codex-app-prod/", ".zip")?;
    let size: u64 = enclosure
        .length
        .parse()
        .context("Некорректный размер архива")?;
    ensure!(
        (1..=MAX_DOWNLOAD).contains(&size),
        "Недопустимый размер архива"
    );
    let signature = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(&enclosure.signature)
            .ok()
            .and_then(|bytes| ed25519_dalek::Signature::from_slice(&bytes).ok())
            .context("Некорректная подпись архива ChatGPT")?
    };
    Ok(Some(Release {
        display_version: version.to_string(),
        version,
        target,
        url,
        integrity: Integrity::Ed25519 {
            signature,
            key: SPARKLE_PUBLIC_KEY,
        },
        size: Some(size),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsFeed {
    schema_version: u32,
    package_version: String,
    packages: Vec<WindowsFeedPackage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsFeedPackage {
    architecture: String,
    download_url: String,
}

fn parse_windows_release(
    bytes: &[u8],
    architecture: &str,
    target: Target,
) -> Result<Option<Release>> {
    let feed: WindowsFeed =
        serde_json::from_slice(bytes).context("Некорректный ответ сервера обновлений")?;
    ensure!(
        feed.schema_version == 1,
        "Неподдерживаемый формат ленты обновлений ChatGPT"
    );
    let version = package_version(&feed.package_version)?;
    if target
        .version
        .as_ref()
        .is_some_and(|current| version <= *current)
    {
        return Ok(None);
    }
    let package = feed
        .packages
        .into_iter()
        .find(|package| package.architecture == architecture)
        .context("В ленте обновлений нет ChatGPT для этой архитектуры Windows")?;
    let url = download_url(
        &package.download_url,
        DOWNLOAD_HOST,
        "/codex-app-prod/releases/",
        ".msix",
    )?;
    Ok(Some(Release {
        display_version: feed.package_version,
        version,
        target,
        url,
        integrity: Integrity::PackageSignature,
        size: None,
    }))
}

/// The application version that `ChatGPT` shows for package `version`, from the Store manifest.
fn application_version(bytes: &[u8], version: &Version) -> Result<Option<String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StoreUpdate {
        build_version: String,
        release_version: String,
        package_identity: String,
    }

    let update: StoreUpdate =
        serde_json::from_slice(bytes).context("Некорректный ответ сервера обновлений")?;
    Ok((update.package_identity == WINDOWS_PACKAGE
        && package_version(&update.build_version).is_ok_and(|build| build == *version)
        && Version::parse(&update.release_version).is_ok())
    .then_some(update.release_version))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "bounded test fixtures")]
mod tests {
    use super::*;

    const SIGNATURE: &str =
        "Qz7NGgktYvJamUjNETrbLvVQ25Loaisn67IsyCLVqcFXm3PKzc1czdptgiGRDmz+o5QnzBxv4joe0FOxvy24CA==";

    fn target(version: Option<&str>) -> Target {
        Target::new(
            DesktopApp::ChatGpt,
            None,
            version.map(|version| Version::parse(version).unwrap()),
        )
    }

    fn item(version: &str, extra: &str, url: &str) -> String {
        format!(
            r#"<item>
            <title>{version}</title>
            <sparkle:version>1</sparkle:version>
            <sparkle:shortVersionString>{version}</sparkle:shortVersionString>
            {extra}
            <enclosure url="{url}" length="813396380" type="application/octet-stream" sparkle:edSignature="{SIGNATURE}" />
            <sparkle:deltas>
                <enclosure url="https://persistent.oaistatic.com/codex-app-prod/delta-{version}.delta" sparkle:deltaFrom="1" length="10" sparkle:edSignature="{SIGNATURE}" />
            </sparkle:deltas>
        </item>"#
        )
    }

    fn appcast(items: &[String]) -> String {
        format!(
            r#"<?xml version='1.0' encoding='utf-8'?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0">
    <channel>
        <title>Codex</title>
        {}
    </channel>
</rss>"#,
            items.join("\n")
        )
    }

    fn zip(version: &str) -> String {
        format!(
            "https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-{version}.zip"
        )
    }

    const ARM: &str = "<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion><sparkle:hardwareRequirements>arm64</sparkle:hardwareRequirements>";

    #[test]
    fn appcast_selects_newest_full_archive_for_this_mac() {
        let feed = appcast(&[
            item("26.1027.61615", ARM, &zip("26.1027.61615")),
            item("26.1029.61003", ARM, &zip("26.1029.61003")),
            item(
                "26.1030.1",
                "<sparkle:channel>beta</sparkle:channel>",
                &zip("26.1030.1"),
            ),
            item(
                "26.1031.1",
                "<sparkle:hardwareRequirements>x86_64</sparkle:hardwareRequirements>",
                &zip("26.1031.1"),
            ),
            item(
                "26.1032.1",
                "<sparkle:minimumSystemVersion>99.0</sparkle:minimumSystemVersion>",
                &zip("26.1032.1"),
            ),
        ]);
        let system = Version::new(15, 5, 0);

        let release = parse_appcast(&feed, &system, "arm64", target(Some("26.915.31945")))
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version::parse("26.1029.61003").unwrap());
        assert_eq!(release.display_version, "26.1029.61003");
        assert_eq!(release.url.as_str(), zip("26.1029.61003"));
        assert_eq!(release.size, Some(813_396_380));
        assert!(matches!(
            release.integrity,
            Integrity::Ed25519 {
                key: SPARKLE_PUBLIC_KEY,
                ..
            }
        ));

        assert!(
            parse_appcast(&feed, &system, "arm64", target(Some("26.1029.61003")))
                .unwrap()
                .is_none()
        );
        assert!(
            parse_appcast(&feed, &system, "arm64", target(None))
                .unwrap()
                .is_some()
        );
        assert!(parse_appcast(&feed, &Version::new(12, 7, 0), "arm64", target(None)).is_err());
    }

    #[test]
    fn intel_macs_skip_apple_silicon_archives() {
        let intel = |version: &str| {
            format!(
                "https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-x64-{version}.zip"
            )
        };
        let feed = appcast(&[
            item(
                "26.1029.61003",
                "<sparkle:minimumSystemVersion>12.0</sparkle:minimumSystemVersion>",
                &intel("26.1029.61003"),
            ),
            item("26.1030.1", ARM, &zip("26.1030.1")),
        ]);
        let system = Version::new(12, 7, 0);

        let release = parse_appcast(&feed, &system, "x86_64", target(None))
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version::parse("26.1029.61003").unwrap());
        assert_eq!(release.url.as_str(), intel("26.1029.61003"));
    }

    #[test]
    fn appcast_rejects_foreign_or_unsigned_archives() {
        let system = Version::new(15, 0, 0);
        for url in [
            "http://persistent.oaistatic.com/codex-app-prod/ChatGPT.zip",
            "https://example.com/codex-app-prod/ChatGPT.zip",
            "https://persistent.oaistatic.com:8443/codex-app-prod/ChatGPT.zip",
            "https://user@persistent.oaistatic.com/codex-app-prod/ChatGPT.zip",
            "https://persistent.oaistatic.com/other/ChatGPT.zip",
            "https://persistent.oaistatic.com/codex-app-prod/ChatGPT.dmg",
        ] {
            let feed = appcast(&[item("26.1029.61003", ARM, url)]);
            assert!(
                parse_appcast(&feed, &system, "arm64", target(None)).is_err(),
                "{url}"
            );
        }

        let unsigned = appcast(&[item("26.1029.61003", ARM, &zip("26.1029.61003")).replace(
            &format!("sparkle:edSignature=\"{SIGNATURE}\" />\n            <sparkle:deltas>"),
            "/>\n            <sparkle:deltas>",
        )]);
        assert!(parse_appcast(&unsigned, &system, "arm64", target(None)).is_err());
        assert!(parse_appcast("<rss><channel>", &system, "arm64", target(None)).is_err());
    }

    #[test]
    fn sparkle_public_key_is_valid() {
        super::super::ed25519_verifying_key(SPARKLE_PUBLIC_KEY).unwrap();
    }

    fn windows_feed(version: &str) -> Vec<u8> {
        serde_json::json!({
            "schemaVersion": 1,
            "releaseDate": "2026-11-04T02:59:50Z",
            "packageVersion": version,
            "packages": [
                {"architecture": "x64", "downloadUrl": format!("https://persistent.oaistatic.com/codex-app-prod/releases/{version}/ChatGPT-x64.msix")},
                {"architecture": "arm64", "downloadUrl": format!("https://persistent.oaistatic.com/codex-app-prod/releases/{version}/ChatGPT-arm64.msix")},
            ],
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn windows_feed_selects_architecture_and_compares_package_versions() {
        let feed = windows_feed("26.1029.7803.0");
        let release = parse_windows_release(&feed, "arm64", target(Some("26.1027.100")))
            .unwrap()
            .unwrap();
        assert_eq!(release.version, Version::new(26, 1029, 7803));
        assert_eq!(release.display_version, "26.1029.7803.0");
        assert!(release.url.path().ends_with("/ChatGPT-arm64.msix"));
        assert!(matches!(release.integrity, Integrity::PackageSignature));
        assert_eq!(release.size, None);

        assert!(
            parse_windows_release(&feed, "x64", target(Some("26.1029.7803")))
                .unwrap()
                .is_none()
        );
        assert!(parse_windows_release(&feed, "x86", target(None)).is_err());
        assert!(
            parse_windows_release(&windows_feed("26.1029.7803.1"), "x64", target(None)).is_err()
        );

        let foreign = String::from_utf8(feed)
            .unwrap()
            .replace("persistent.oaistatic.com", "example.com");
        assert!(parse_windows_release(foreign.as_bytes(), "x64", target(None)).is_err());
    }

    #[test]
    fn store_manifest_names_the_application_version() {
        let manifest = br#"{"schemaVersion":1,"buildVersion":"26.1029.7803.0","storeProductId":"9PLM9XGG6VKS","packageIdentity":"OpenAI.Codex","releaseVersion":"26.1029.61003"}"#;
        assert_eq!(
            application_version(manifest, &Version::new(26, 1029, 7803)).unwrap(),
            Some("26.1029.61003".to_owned())
        );
        assert_eq!(
            application_version(manifest, &Version::new(26, 1029, 7804)).unwrap(),
            None
        );
    }

    #[cfg(target_os = "macos")]
    fn bundle(path: &Path, id: &str, version: &str) {
        std::fs::create_dir_all(path.join("Contents")).unwrap();
        std::fs::write(
            path.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_layout_updates_migrates_and_moves_classic_aside() {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("Applications");
        let user = root.path().join("home/Applications");
        let directories = [system.clone(), user.clone()];
        std::fs::create_dir_all(&system).unwrap();
        std::fs::create_dir_all(&user).unwrap();

        // Nothing installed: a fresh /Applications/ChatGPT.app.
        let fresh = detect_in(&directories).unwrap();
        assert_eq!(fresh.path, Some(system.join("ChatGPT.app")));
        assert_eq!(fresh.version, None);
        assert!(!fresh.replaces && fresh.rename.is_none() && fresh.superseded.is_empty());

        // Only an old Codex.app in the user's folder: install next to it and supersede it.
        bundle(&user.join("Codex.app"), BUNDLE_ID, "26.915.31945");
        let migrated = detect_in(&directories).unwrap();
        assert_eq!(migrated.path, Some(user.join("ChatGPT.app")));
        assert_eq!(migrated.version, Some(Version::new(26, 915, 31945)));
        assert!(!migrated.replaces);
        assert_eq!(migrated.superseded, [user.join("Codex.app")]);

        // ChatGPT Classic occupies the name: move it aside after confirmation.
        std::fs::remove_dir_all(user.join("Codex.app")).unwrap();
        bundle(&system.join("ChatGPT.app"), CLASSIC_BUNDLE_ID, "1.2026.184");
        let classic = detect_in(&directories).unwrap();
        assert!(!classic.replaces);
        let rename = classic.rename.unwrap();
        assert_eq!(rename.from, system.join("ChatGPT.app"));
        assert_eq!(rename.to, system.join("ChatGPT Classic.app"));

        bundle(
            &system.join("ChatGPT Classic.app"),
            CLASSIC_BUNDLE_ID,
            "1.0",
        );
        assert!(detect_in(&directories).is_err());

        // The current ChatGPT elsewhere is updated in place; Classic stays untouched.
        bundle(&user.join("ChatGPT.app"), BUNDLE_ID, "26.1027.61615");
        bundle(&system.join("Codex.app"), BUNDLE_ID, "26.915.31945");
        let current = detect_in(&directories).unwrap();
        assert_eq!(current.path, Some(user.join("ChatGPT.app")));
        assert_eq!(current.version, Some(Version::new(26, 1027, 61615)));
        assert!(current.replaces && current.rename.is_none());
        assert_eq!(current.superseded, [system.join("Codex.app")]);

        // Another application under the name is never replaced.
        std::fs::remove_dir_all(user.join("ChatGPT.app")).unwrap();
        std::fs::remove_dir_all(system.join("Codex.app")).unwrap();
        std::fs::remove_dir_all(system.join("ChatGPT.app")).unwrap();
        bundle(&system.join("ChatGPT.app"), "com.example.other", "1.0");
        assert!(detect_in(&directories).is_err());
    }

    #[test]
    fn versions_parse_strictly() {
        assert_eq!(
            package_version("26.1029.7803.0").unwrap(),
            Version::new(26, 1029, 7803)
        );
        for invalid in ["26.1029.7803", "26.1029.7803.1", "26.x.1.0", ""] {
            assert!(package_version(invalid).is_err(), "{invalid}");
        }
        assert_eq!(lenient_version("13.0"), Some(Version::new(13, 0, 0)));
        assert_eq!(lenient_version("15.5.1"), Some(Version::new(15, 5, 1)));
        assert_eq!(lenient_version("15.5.1.2"), None);
        assert_eq!(lenient_version("beta"), None);
    }
}
