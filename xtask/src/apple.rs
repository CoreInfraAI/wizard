use crate::utils::{require_success, required_env};
use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use std::{
    env,
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};
use tempfile::NamedTempFile;

/// Apple Developer ID credentials used to sign and notarize macOS bundles.
pub(crate) struct Credentials {
    team_id: String,
    certificate: String,
    certificate_password: String,
    api_issuer: String,
    api_key_id: String,
    api_key: NamedTempFile,
}

impl Credentials {
    /// Reads the credentials from the release workflow secrets.
    pub(crate) fn from_env() -> Result<Self> {
        let team_id = required_env("APPLE_TEAM_ID")?;
        let certificate = required_env("APPLE_DEVELOPER_ID_CERT_P12")?;
        let certificate_password = required_env("APPLE_DEVELOPER_ID_CERT_PASSWORD")?;
        let api_issuer = required_env("APPLE_NOTARY_API_ISSUER_ID")?;
        let api_key_id = required_env("APPLE_NOTARY_API_KEY_ID")?;
        let api_key_pem = required_env("APPLE_NOTARY_API_KEY")?;
        if !api_key_pem
            .trim_start()
            .starts_with("-----BEGIN PRIVATE KEY-----")
        {
            bail!("APPLE_NOTARY_API_KEY must contain the .p8 private key in PEM format");
        }

        // Tauri and notarytool only accept the API key as a file.
        let mut api_key = tempfile::Builder::new()
            .suffix(".p8")
            .tempfile()
            .context("failed to create notary API key file")?;
        api_key
            .write_all(api_key_pem.as_bytes())
            .context("failed to write notary API key file")?;

        Ok(Self {
            team_id,
            certificate,
            certificate_password,
            api_issuer,
            api_key_id,
            api_key,
        })
    }

    /// Configures `tauri bundle` to sign with the Developer ID certificate and notarize the app.
    pub(crate) fn apply(&self, command: &mut Command) {
        command
            .env("APPLE_CERTIFICATE", &self.certificate)
            .env("APPLE_CERTIFICATE_PASSWORD", &self.certificate_password)
            // Overrides the ad-hoc identity in tauri.conf.json. Tauri checks that the
            // imported certificate's name contains it.
            .env("APPLE_SIGNING_IDENTITY", "Developer ID Application")
            .env("APPLE_API_ISSUER", &self.api_issuer)
            .env("APPLE_API_KEY", &self.api_key_id)
            .env("APPLE_API_KEY_PATH", self.api_key.path());
    }

    fn notarytool(&self, args: &[&str]) -> Command {
        let mut command = Command::new("xcrun");
        command
            .arg("notarytool")
            .args(args)
            .arg("--key")
            .arg(self.api_key.path())
            .args(["--key-id", &self.api_key_id, "--issuer", &self.api_issuer]);
        command
    }
}

/// Removes Apple signing variables, including Tauri's, so that only an explicitly
/// configured `tauri bundle` signs with Developer ID.
pub(crate) fn remove_env(command: &mut Command) {
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("APPLE_") {
            command.env_remove(key);
        }
    }
}

/// Notarizes a file and staples the ticket to it.
pub(crate) fn notarize(credentials: &Credentials, path: &Path) -> Result<()> {
    let output = credentials
        .notarytool(&[
            "submit",
            "--wait",
            "--timeout",
            "30m",
            "--output-format",
            "json",
        ])
        .arg(path)
        .stderr(Stdio::inherit())
        .output()
        .context("failed to run notarytool")?;
    let submission: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("notarytool submit exited with {}", output.status))?;
    let id = submission["id"]
        .as_str()
        .context("notarytool submit returned no submission ID")?;

    // notarytool can exit successfully for rejected submissions.
    let status = submission["status"].as_str().unwrap_or("unknown");
    if status != "Accepted" {
        drop(credentials.notarytool(&["log", id]).status());
        bail!(
            "notarization {id} of {} finished with status {status}",
            path.display()
        );
    }

    let status = Command::new("xcrun")
        .args(["stapler", "staple"])
        .arg(path)
        .status()
        .context("failed to run stapler")?;
    require_success(status, "stapler staple")
}

/// Verifies that the disk image and the app in the updater archive pass Gatekeeper
/// as downloaded copies would. Both are built from the same stapled app.
pub(crate) fn verify(
    credentials: &Credentials,
    dmg: &Path,
    updater: &Path,
    app_name: &str,
) -> Result<()> {
    let team_id = &credentials.team_id;
    verify_team(dmg, team_id)?;
    validate_staple(dmg)?;
    assess(
        dmg,
        &["--type", "open", "--context", "context:primary-signature"],
    )?;

    let temporary = tempfile::tempdir().context("failed to create temporary directory")?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(updater)
        .arg("-C")
        .arg(temporary.path())
        .status()
        .context("failed to run tar")?;
    require_success(status, "updater archive extraction")?;
    let app = temporary.path().join(app_name);
    verify_team(&app, team_id)?;
    validate_staple(&app)?;
    assess(&app, &["--type", "execute"])
}

/// Checks the signing team. Gatekeeper's notarization check covers the rest of the signature.
fn verify_team(path: &Path, team_id: &str) -> Result<()> {
    let output = Command::new("codesign")
        .args(["--display", "--verbose"])
        .arg(path)
        .output()
        .context("failed to run codesign")?;
    require_success(output.status, "codesign --display")?;
    let expected = format!("TeamIdentifier={team_id}");
    if !String::from_utf8_lossy(&output.stderr)
        .lines()
        .any(|line| line == expected)
    {
        bail!("{} is not signed by team {team_id}", path.display());
    }
    Ok(())
}

fn validate_staple(path: &Path) -> Result<()> {
    let status = Command::new("xcrun")
        .args(["stapler", "validate"])
        .arg(path)
        .status()
        .context("failed to run stapler")?;
    require_success(status, "stapler validate")
}

fn assess(path: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("spctl")
        .arg("--assess")
        .args(args)
        .arg("--verbose=2")
        .arg(path)
        .output()
        .context("failed to run spctl")?;
    let verdict = String::from_utf8_lossy(&output.stderr);
    print!("{verdict}");
    require_success(output.status, "Gatekeeper assessment")?;
    if !verdict
        .lines()
        .any(|line| line == "source=Notarized Developer ID")
    {
        bail!("Gatekeeper did not accept {} as notarized", path.display());
    }
    Ok(())
}
