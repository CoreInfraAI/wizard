use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use serde_json::{Value, json};

use crate::dev_tag;
use crate::utils::{
    Paths, clean_build_command, gh_api_bytes, gh_api_json, gh_release_upload, remove_path,
    require_success, required_env, stable_version,
};

const DEV_ENDPOINT: &str = "https://coreinfraai.github.io/wizard/latest-dev.json";
const DEV_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMyMEE1QkNBNDdCREYyQUEKUldTcThyMUh5bHNLTWtyVU5LYWtZcmI4VE1QRTZvbHF6K1daUHByNDZsd2VIUHBjWWV5cFVrazgK";

/// Finds or creates the draft release used by the release workflow.
pub(crate) fn create(paths: &Paths, dev: bool) -> Result<()> {
    let repository = required_env("GITHUB_REPOSITORY")?;
    let commit = required_env("GITHUB_SHA")?;
    let version = if dev {
        dev_tag::get_or_create(paths, &repository, &commit)?
    } else {
        stable_version(paths)?
    };
    let expected_tag = format!("v{version}");
    let tag = if dev {
        expected_tag
    } else {
        let tag = required_env("GITHUB_REF_NAME")?;
        if tag != expected_tag {
            bail!("release tag is {tag}, expected {expected_tag}");
        }
        tag
    };

    let release_id = if let Some(release) = find_github_release_by_tag(&repository, &tag)? {
        if release["draft"].as_bool() != Some(true) {
            bail!("release {tag} is already published");
        }
        if release["prerelease"].as_bool() != Some(dev) {
            bail!("release {tag} has an unexpected prerelease status");
        }
        release["id"]
            .as_u64()
            .context("GitHub release has no numeric ID")?
    } else {
        let config: Value = serde_json::from_slice(&fs::read(&paths.tauri_config)?)?;
        let product_name = config["productName"]
            .as_str()
            .context("tauri.conf.json productName must be a string")?;
        let body = if dev {
            format!(
                "Development build from commit `{commit}` on the `dev` branch.\n\n\
                 This prerelease may be unstable and is intended for testing only."
            )
        } else {
            "Release builds for Linux, macOS, and Windows.\n\n\
             See the generated release notes below for the complete list of changes."
                .to_owned()
        };
        create_github_release(
            &repository,
            &tag,
            &commit,
            &format!("{product_name} {version}"),
            &body,
            dev,
        )?
    };

    let output_path = required_env("GITHUB_OUTPUT")?;
    let mut output = fs::OpenOptions::new()
        .append(true)
        .open(&output_path)
        .with_context(|| format!("failed to open {output_path}"))?;
    writeln!(output, "release_id={release_id}")?;
    writeln!(output, "version={version}")?;
    writeln!(output, "tag={tag}")?;
    Ok(())
}

fn create_github_release(
    repository: &str,
    tag: &str,
    commit: &str,
    name: &str,
    body: &str,
    prerelease: bool,
) -> Result<u64> {
    let output = Command::new("gh")
        .args([
            "api",
            "--method",
            "POST",
            &format!("repos/{repository}/releases"),
            "-f",
            &format!("tag_name={tag}"),
            "-f",
            &format!("target_commitish={commit}"),
            "-f",
            &format!("name={name}"),
            "-f",
            &format!("body={body}"),
            "-F",
            "draft=true",
            "-F",
            &format!("prerelease={prerelease}"),
            "-F",
            "generate_release_notes=true",
        ])
        .output()
        .context("failed to run gh api")?;
    if !output.status.success() {
        bail!(
            "failed to create release {tag}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let release: Value =
        serde_json::from_slice(&output.stdout).context("gh api returned invalid JSON")?;
    if release["tag_name"].as_str() != Some(tag) {
        bail!("created GitHub release has an unexpected tag");
    }
    release["id"]
        .as_u64()
        .context("created GitHub release has no numeric ID")
}

fn find_github_release_by_tag(repository: &str, tag: &str) -> Result<Option<Value>> {
    let mut page = 1_u64;
    loop {
        let releases = gh_api_json(&format!(
            "repos/{repository}/releases?per_page=100&page={page}"
        ))?;
        let releases = releases
            .as_array()
            .context("GitHub releases response must be an array")?;
        if let Some(release) = releases
            .iter()
            .find(|release| release["tag_name"].as_str() == Some(tag))
        {
            return Ok(Some(release.clone()));
        }
        if releases.len() < 100 {
            return Ok(None);
        }
        page = page
            .checked_add(1)
            .context("release page number overflow")?;
    }
}

fn release_version(paths: &Paths, dev: bool, version: Option<&str>) -> Result<semver::Version> {
    match (dev, version) {
        (true, Some(version)) => dev_tag::parse_version(paths, version),
        (true, None) => bail!("--version must be set with --dev"),
        (false, Some(_)) => bail!("--version can only be used with --dev"),
        (false, None) => stable_version(paths),
    }
}

/// Builds and stages release assets using stable, predictable names.
pub(crate) fn build(
    paths: &Paths,
    dev: bool,
    version: Option<&str>,
    target: &str,
    output: &Path,
) -> Result<()> {
    let platform = match target {
        "x86_64-unknown-linux-gnu" => "linux",
        "aarch64-apple-darwin" => "darwin",
        "x86_64-pc-windows-msvc" => "windows",
        _ => bail!("unsupported release target: {target}"),
    };

    // Validate this before the expensive build, but never pass it to the build process.
    drop(required_env("TAURI_SIGNING_PRIVATE_KEY")?);

    let version = release_version(paths, dev, version)?;
    let config: Value = serde_json::from_slice(&fs::read(&paths.tauri_config)?)?;
    let product_name = config["productName"]
        .as_str()
        .context("tauri.conf.json productName must be a string")?;
    let updater_public_key = if dev {
        DEV_PUBLIC_KEY
    } else {
        config["plugins"]["updater"]["pubkey"]
            .as_str()
            .context("tauri.conf.json updater pubkey must be a string")?
    };

    let bundle_dir = paths
        .workspace
        .join("target")
        .join(target)
        .join("release")
        .join("bundle");
    remove_path(&bundle_dir)?;
    remove_path(output)?;

    // The workflow builds the frontend and xtask before exposing the signing key.
    // Disable Tauri's frontend hook and remove the key from the Rust build process. The
    // separate bundling process receives the key only to sign the finished artifacts.
    let build_config = if dev {
        json!({
            "version": version.to_string(),
            "build": {
                "beforeBuildCommand": "",
                "beforeBundleCommand": "",
            },
            "plugins": {
                "updater": {
                    "endpoints": [DEV_ENDPOINT],
                    "pubkey": DEV_PUBLIC_KEY,
                },
            },
        })
    } else {
        json!({
            "build": {
                "beforeBuildCommand": "",
                "beforeBundleCommand": "",
            },
        })
    }
    .to_string();
    let bundles: &[&str] = match target {
        "aarch64-apple-darwin" => &["app", "dmg"],
        "x86_64-unknown-linux-gnu" => &["appimage", "deb", "rpm"],
        "x86_64-pc-windows-msvc" => &["nsis"],
        _ => bail!("unsupported release target: {target}"),
    };
    let tauri_cli = "wizard-ui/node_modules/@tauri-apps/cli/tauri.js";
    let mut build_command = clean_build_command("node");
    build_command
        .arg(tauri_cli)
        .args(["build", "--target", target, "--no-bundle"])
        .args(["--config", &build_config])
        .env_remove("TAURI_SIGNING_PRIVATE_KEY")
        .env_remove("TAURI_SIGNING_PRIVATE_KEY_PASSWORD")
        .env_remove("TAURI_SIGNING_PRIVATE_KEY_PATH")
        .env_remove("TAURI_PRIVATE_KEY")
        .env_remove("TAURI_PRIVATE_KEY_PASSWORD")
        .env_remove("TAURI_PRIVATE_KEY_PATH")
        .current_dir(&paths.wizard);
    require_success(build_command.status()?, "tauri build")?;

    let mut bundle_command = clean_build_command("node");
    bundle_command
        .arg(tauri_cli)
        .args(["bundle", "--target", target, "--bundles"])
        .args(bundles)
        .args(["--config", &build_config])
        .current_dir(&paths.wizard);
    if platform == "darwin" {
        // better looking .dmg
        bundle_command.env("TAURI_BUNDLER_DMG_IGNORE_CI", "true");
    }
    require_success(bundle_command.status()?, "tauri bundle")?;

    if platform == "darwin" {
        verify_macos_app_signature(&bundle_dir.join("macos").join(format!("{product_name}.app")))?;
    }

    let updater_artifact = match target {
        "aarch64-apple-darwin" => bundle_dir
            .join("macos")
            .join(format!("{product_name}.app.tar.gz")),
        "x86_64-unknown-linux-gnu" => bundle_dir
            .join("appimage")
            .join(format!("{product_name}_{version}_amd64.AppImage")),
        "x86_64-pc-windows-msvc" => bundle_dir
            .join("nsis")
            .join(format!("{product_name}_{version}_x64-setup.exe")),
        _ => bail!("unsupported release target: {target}"),
    };
    verify_updater_signature(&updater_artifact, updater_public_key)?;

    fs::create_dir_all(output).with_context(|| format!("failed to create {}", output.display()))?;
    let release_name = format!("{product_name}-{version}");
    let copy =
        |source, destination| move_artifact(&bundle_dir.join(source), &output.join(destination));

    match target {
        "aarch64-apple-darwin" => {
            copy(
                format!("dmg/{product_name}_{version}_aarch64.dmg"),
                format!("{release_name}-darwin-aarch64-install.dmg"),
            )?;
            copy(
                format!("macos/{product_name}.app.tar.gz"),
                format!("{release_name}-darwin-aarch64-update.app.tar.gz"),
            )?;
            copy(
                format!("macos/{product_name}.app.tar.gz.sig"),
                format!("{release_name}-darwin-aarch64-update.app.tar.gz.sig"),
            )
        }
        "x86_64-unknown-linux-gnu" => {
            let app_image = format!("{product_name}_{version}_amd64.AppImage");
            copy(
                format!("appimage/{app_image}"),
                format!("{release_name}-linux-amd64.AppImage"),
            )?;
            copy(
                format!("appimage/{app_image}.sig"),
                format!("{release_name}-linux-amd64.AppImage.sig"),
            )?;
            copy(
                format!("deb/{product_name}_{version}_amd64.deb"),
                format!("{release_name}-linux-amd64.deb"),
            )?;
            copy(
                format!("rpm/{product_name}-{version}-1.x86_64.rpm"),
                format!("{release_name}-linux-x86_64.rpm"),
            )
        }
        "x86_64-pc-windows-msvc" => {
            let installer = format!("{product_name}_{version}_x64-setup.exe");
            copy(
                format!("nsis/{installer}"),
                format!("{release_name}-windows-x64.exe"),
            )?;
            copy(
                format!("nsis/{installer}.sig"),
                format!("{release_name}-windows-x64.exe.sig"),
            )
        }
        _ => bail!("unsupported release target: {target}"),
    }
}

fn verify_macos_app_signature(app: &Path) -> Result<()> {
    if !app.is_dir() {
        bail!(
            "expected macOS application was not generated: {}",
            app.display()
        );
    }

    let status = Command::new("codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=4"])
        .arg(app)
        .status()
        .context("failed to run codesign")?;
    require_success(status, "codesign verification")
}

fn verify_updater_signature(artifact: &Path, public_key: &str) -> Result<()> {
    if !artifact.is_file() {
        bail!(
            "expected updater artifact was not generated: {}",
            artifact.display()
        );
    }

    let signature_path = signature_path(artifact);
    let signature = fs::read_to_string(&signature_path)
        .with_context(|| format!("failed to read {}", signature_path.display()))?;
    let public_key = decode_minisign(public_key, "updater public key")?;
    let public_key = PublicKey::decode(&public_key).context("invalid updater public key")?;
    let signature = decode_minisign(&signature, "updater signature")?;
    let signature = Signature::decode(&signature).context("invalid updater signature")?;
    let artifact_bytes =
        fs::read(artifact).with_context(|| format!("failed to read {}", artifact.display()))?;
    public_key
        .verify(&artifact_bytes, &signature, false)
        .with_context(|| {
            format!(
                "updater signature does not match the configured public key for {}",
                artifact.display()
            )
        })
}

fn signature_path(artifact: &Path) -> PathBuf {
    let mut path = artifact.as_os_str().to_os_string();
    path.push(".sig");
    path.into()
}

fn decode_minisign(value: &str, description: &str) -> Result<String> {
    let decoded = STANDARD
        .decode(value.trim())
        .with_context(|| format!("{description} is not valid base64"))?;
    String::from_utf8(decoded).with_context(|| format!("decoded {description} is not valid UTF-8"))
}

fn move_artifact(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_file() {
        bail!(
            "expected release artifact was not generated: {}",
            source.display()
        );
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy from {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    println!("artifact: {}", destination.display());
    Ok(())
}

/// Generates `latest.json` from the complete set of assets in a GitHub draft release.
pub(crate) fn generate_latest_json(
    paths: &Paths,
    dev: bool,
    repository: &str,
    release_id: u64,
) -> Result<()> {
    if repository.split('/').count() != 2 {
        bail!("repository must use owner/name format");
    }

    let release = gh_api_json(&format!("repos/{repository}/releases/{release_id}"))?;
    let tag = release["tag_name"]
        .as_str()
        .context("GitHub release has no tag_name")?;
    let version = if dev {
        let value = tag
            .strip_prefix('v')
            .context("dev release tag must start with v")?;
        dev_tag::parse_version(paths, value)?
    } else {
        stable_version(paths)?
    };
    let expected_tag = format!("v{version}");
    if tag != expected_tag {
        bail!("release tag {tag} does not match application version; expected {expected_tag}");
    }

    if release["draft"].as_bool() != Some(true) {
        bail!("release {tag} must still be a draft");
    }
    if release["prerelease"].as_bool() != Some(dev) {
        bail!("release {tag} has an unexpected prerelease status");
    }
    let notes = release["body"].as_str().unwrap_or_default();
    let pub_date = release["created_at"]
        .as_str()
        .context("GitHub release has no created_at timestamp")?;
    let assets = release["assets"]
        .as_array()
        .context("GitHub release assets must be an array")?;
    let config: Value = serde_json::from_slice(&fs::read(&paths.tauri_config)?)?;
    let product_name = config["productName"]
        .as_str()
        .context("tauri.conf.json productName must be a string")?;
    let release_name = format!("{product_name}-{version}");
    let platforms = json!({
        "darwin-aarch64": updater_entry(
            assets,
            repository,
            tag,
            &format!("{release_name}-darwin-aarch64-update.app.tar.gz.sig"),
        )?,
        "linux-x86_64": updater_entry(
            assets,
            repository,
            tag,
            &format!("{release_name}-linux-amd64.AppImage.sig"),
        )?,
        "windows-x86_64": updater_entry(
            assets,
            repository,
            tag,
            &format!("{release_name}-windows-x64.exe.sig"),
        )?,
    });

    let latest = json!({
        "version": version.to_string(),
        "notes": notes,
        "pub_date": pub_date,
        "platforms": platforms,
    });
    let temporary = tempfile::tempdir().context("failed to create temporary directory")?;
    let output = temporary.path().join("latest.json");
    fs::write(&output, serde_json::to_vec_pretty(&latest)?)
        .with_context(|| format!("failed to write {}", output.display()))?;
    gh_release_upload(repository, tag, &output)
}

fn updater_entry(
    assets: &[Value],
    repository: &str,
    tag: &str,
    signature_name: &str,
) -> Result<Value> {
    let signature_asset = assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(signature_name))
        .with_context(|| format!("signature asset was not uploaded: {signature_name}"))?;
    let updater_name = signature_name
        .strip_suffix(".sig")
        .context("updater signature filename must end in .sig")?;
    assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(updater_name))
        .with_context(|| format!("updater asset was not uploaded: {updater_name}"))?;
    let url = release_asset_url(repository, tag, updater_name);
    let signature_id = signature_asset["id"]
        .as_u64()
        .with_context(|| format!("signature asset {signature_name} has no numeric ID"))?;
    let signature = gh_api_bytes(&format!(
        "repos/{repository}/releases/assets/{signature_id}"
    ))?;
    let signature = String::from_utf8(signature)
        .with_context(|| format!("signature asset {signature_name} is not UTF-8"))?;

    Ok(json!({
        "signature": signature.trim(),
        "url": url,
    }))
}

fn release_asset_url(repository: &str, tag: &str, asset_name: &str) -> String {
    format!("https://github.com/{repository}/releases/download/{tag}/{asset_name}")
}
