use std::{env, fs, path::Path, process::Command};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::utils::{
    Paths, gh_api_bytes, gh_api_json, gh_release_upload, remove_path, require_success,
    stable_version,
};

/// Builds production bundles and stages release assets using stable, predictable names.
pub(crate) fn build(paths: &Paths, target: &str, output: &Path) -> Result<()> {
    let platform = match target {
        "x86_64-unknown-linux-gnu" => "linux",
        "aarch64-apple-darwin" | "x86_64-apple-darwin" => "darwin",
        "x86_64-pc-windows-msvc" => "windows",
        _ => bail!("unsupported release target: {target}"),
    };

    let signing_key = env::var_os("TAURI_SIGNING_PRIVATE_KEY")
        .context("TAURI_SIGNING_PRIVATE_KEY must be set for release-build")?;
    let version = stable_version(paths)?;
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

    let mut command = Command::new("node");
    command
        .arg("ff-wizard-ui/node_modules/@tauri-apps/cli/tauri.js")
        .args(["build", "--target", target])
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
pub(crate) fn generate_latest_json(paths: &Paths, repository: &str, tag: &str) -> Result<()> {
    if repository.split('/').count() != 2 {
        bail!("repository must use owner/name format");
    }

    let version = stable_version(paths)?;
    let expected_tag = format!("v{version}");
    if tag != expected_tag {
        bail!("release tag {tag} does not match application version; expected {expected_tag}");
    }

    let release = gh_api_json(&format!("repos/{repository}/releases/tags/{tag}"))?;
    if release["draft"].as_bool() != Some(true) {
        bail!("release {tag} must still be a draft");
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
