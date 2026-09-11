use std::{env, fs, io::Write as _, path::Path, process::Command};

use anyhow::{Context as _, Result, bail};
use semver::Version;
use serde_json::{Value, json};

use crate::utils::{
    Paths, gh_api_bytes, gh_api_json, gh_api_json_optional, gh_release_upload, remove_path,
    require_success, stable_version,
};

const DEV_ENDPOINT: &str = "https://coreinfraai.github.io/wizard/latest-dev.json";

/// Finds or creates the draft release used by the release workflow.
pub(crate) fn create(paths: &Paths, dev: bool) -> Result<()> {
    let repository = required_env("GITHUB_REPOSITORY")?;
    let commit = required_env("GITHUB_SHA")?;
    let version = release_version(paths, dev)?;
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

    let endpoint = format!("repos/{repository}/releases/tags/{tag}");
    let release_id = if let Some(release) = gh_api_json_optional(&endpoint)? {
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
    release["id"]
        .as_u64()
        .context("created GitHub release has no numeric ID")
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} must be set"))
}

fn release_version(paths: &Paths, dev: bool) -> Result<Version> {
    let stable = stable_version(paths)?;
    if !dev {
        return Ok(stable);
    }

    let run_number: u64 = required_env("GITHUB_RUN_NUMBER")?
        .parse()
        .context("GITHUB_RUN_NUMBER must be an integer")?;
    if run_number > 65_535 {
        bail!("GITHUB_RUN_NUMBER exceeds the MSI limit of 65535");
    }
    let patch = stable
        .patch
        .checked_add(1)
        .context("patch version overflow")?;
    Version::parse(&format!(
        "{}.{}.{patch}-{run_number}",
        stable.major, stable.minor
    ))
    .context("failed to construct development version")
}

/// Builds and stages release assets using stable, predictable names.
pub(crate) fn build(paths: &Paths, dev: bool, target: &str, output: &Path) -> Result<()> {
    let platform = match target {
        "x86_64-unknown-linux-gnu" => "linux",
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => "darwin",
        "x86_64-pc-windows-msvc" => "windows",
        _ => bail!("unsupported release target: {target}"),
    };

    let signing_key = env::var_os("TAURI_SIGNING_PRIVATE_KEY")
        .context("TAURI_SIGNING_PRIVATE_KEY must be set for release-build")?;
    let version = release_version(paths, dev)?;
    let config: Value = serde_json::from_slice(&fs::read(&paths.tauri_config)?)?;
    let product_name = config["productName"]
        .as_str()
        .context("tauri.conf.json productName must be a string")?;

    let bundle_dir = paths
        .workspace
        .join("target")
        .join(target)
        .join("release")
        .join("bundle");
    remove_path(&bundle_dir)?;
    remove_path(output)?;

    let dev_config = dev.then(|| {
        json!({
            "version": version.to_string(),
            "plugins": { "updater": { "endpoints": [DEV_ENDPOINT] } },
        })
        .to_string()
    });
    let mut command = Command::new("node");
    command
        .arg("ff-wizard-ui/node_modules/@tauri-apps/cli/tauri.js")
        .args(["build", "--target", target]);
    if let Some(dev_config) = &dev_config {
        command.args(["--config", dev_config]);
    }
    command
        .env("TAURI_SIGNING_PRIVATE_KEY", signing_key)
        .current_dir(&paths.wizard);
    if platform == "darwin" {
        // better looking .dmg
        command.env("TAURI_BUNDLER_DMG_IGNORE_CI", "true");
    }
    require_success(command.status()?, "tauri build")?;

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
        "x86_64-apple-darwin" => {
            copy(
                format!("dmg/{product_name}_{version}_x64.dmg"),
                format!("{release_name}-darwin-x64-install.dmg"),
            )?;
            copy(
                format!("macos/{product_name}.app.tar.gz"),
                format!("{release_name}-darwin-x64-update.app.tar.gz"),
            )?;
            copy(
                format!("macos/{product_name}.app.tar.gz.sig"),
                format!("{release_name}-darwin-x64-update.app.tar.gz.sig"),
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
    tag: &str,
) -> Result<()> {
    if repository.split('/').count() != 2 {
        bail!("repository must use owner/name format");
    }

    let version = release_version(paths, dev)?;
    let expected_tag = format!("v{version}");
    if tag != expected_tag {
        bail!("release tag {tag} does not match application version; expected {expected_tag}");
    }

    let release = gh_api_json(&format!("repos/{repository}/releases/tags/{tag}"))?;
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
            &format!("{release_name}-darwin-aarch64-update.app.tar.gz.sig"),
        )?,
        "darwin-x86_64": updater_entry(
            assets,
            repository,
            &format!("{release_name}-darwin-x64-update.app.tar.gz.sig"),
        )?,
        "linux-x86_64": updater_entry(
            assets,
            repository,
            &format!("{release_name}-linux-amd64.AppImage.sig"),
        )?,
        "windows-x86_64": updater_entry(
            assets,
            repository,
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

fn updater_entry(assets: &[Value], repository: &str, signature_name: &str) -> Result<Value> {
    let signature_asset = assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(signature_name))
        .with_context(|| format!("signature asset was not uploaded: {signature_name}"))?;
    let updater_name = signature_name
        .strip_suffix(".sig")
        .context("updater signature filename must end in .sig")?;
    let updater_asset = assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(updater_name))
        .with_context(|| format!("updater asset was not uploaded: {updater_name}"))?;
    let url = updater_asset["browser_download_url"]
        .as_str()
        .with_context(|| format!("updater asset {updater_name} has no download URL"))?;
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
