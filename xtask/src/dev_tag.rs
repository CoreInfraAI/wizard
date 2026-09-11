use std::process::Command;

use anyhow::{Context as _, Result, bail};
use semver::Version;
use serde_json::Value;

use crate::utils::{Paths, gh_api_json, required_env, stable_version};

struct DevTag {
    version: Version,
    sequence: u64,
    commit: String,
}

pub(crate) fn get_or_create(paths: &Paths, repository: &str, commit: &str) -> Result<Version> {
    let stable = stable_version(paths)?;
    for _ in 0..10 {
        let tags = find(repository, &stable)?;
        if let Some(tag) = tags
            .iter()
            .filter(|tag| tag.commit == commit)
            .max_by_key(|tag| tag.sequence)
        {
            return Ok(tag.version.clone());
        }

        let sequence = tags
            .iter()
            .map(|tag| tag.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("dev tag sequence overflow")?;
        let version = Version::parse(&format!("{stable}-dev.{sequence}"))?;
        if create(repository, &format!("v{version}"), commit)? {
            return Ok(version);
        }
        std::thread::sleep(core::time::Duration::from_secs(1));
    }
    bail!("failed to allocate a unique dev tag after 10 attempts")
}

pub(crate) fn current(paths: &Paths) -> Result<Version> {
    let stable = stable_version(paths)?;
    let repository = required_env("GITHUB_REPOSITORY")?;
    let commit = required_env("GITHUB_SHA")?;
    find(&repository, &stable)?
        .into_iter()
        .filter(|tag| tag.commit == commit)
        .max_by_key(|tag| tag.sequence)
        .map(|tag| tag.version)
        .context("current commit has no dev tag; create-release --dev must run first")
}

fn find(repository: &str, stable: &Version) -> Result<Vec<DevTag>> {
    let prefix = format!("v{stable}-dev.");
    let mut tags = Vec::new();
    for page in 1.. {
        let endpoint =
            format!("repos/{repository}/git/matching-refs/tags/{prefix}?per_page=100&page={page}");
        let response = gh_api_json(&endpoint)?;
        let refs = response
            .as_array()
            .context("GitHub matching refs response must be an array")?;
        for reference in refs {
            let Some(sequence) = reference["ref"]
                .as_str()
                .and_then(|value| value.strip_prefix("refs/tags/"))
                .and_then(|value| value.strip_prefix(&prefix))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            let Some(commit) = reference["object"]["sha"].as_str() else {
                continue;
            };
            tags.push(DevTag {
                version: Version::parse(&format!("{stable}-dev.{sequence}"))?,
                sequence,
                commit: commit.to_owned(),
            });
        }
        if refs.len() < 100 {
            break;
        }
    }
    Ok(tags)
}

fn create(repository: &str, tag: &str, commit: &str) -> Result<bool> {
    let output = Command::new("gh")
        .arg("api")
        .args(["--method", "POST"])
        .arg(format!("repos/{repository}/git/refs"))
        .args(["-f", &format!("ref=refs/tags/{tag}")])
        .args(["-f", &format!("sha={commit}")])
        .output()
        .context("failed to run gh api")?;
    if output.status.success() {
        return Ok(true);
    }
    let response: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    let conflict = response.as_ref().is_some_and(|value| {
        value["status"].as_str() == Some("422") || value["status"].as_u64() == Some(422)
    }) || String::from_utf8_lossy(&output.stderr).contains("HTTP 422");
    if conflict {
        return Ok(false);
    }
    bail!(
        "failed to create tag {tag}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}
