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

/// How long `release-notarize` polls Apple, leaving room within the job timeout
/// for setup, verification, and upload.
const NOTARIZATION_WAIT: &str = "165m";

/// Apple Developer ID certificate used to sign macOS bundles.
pub(crate) struct Signing {
    certificate: String,
    certificate_password: String,
}

impl Signing {
    /// Reads the certificate from the release workflow secrets.
    pub(crate) fn from_env() -> Result<Self> {
        Ok(Self {
            certificate: required_env("APPLE_DEVELOPER_ID_CERT_P12")?,
            certificate_password: required_env("APPLE_DEVELOPER_ID_CERT_PASSWORD")?,
        })
    }

    /// Configures `tauri bundle` to sign with the Developer ID certificate.
    ///
    /// Tauri gets no notary credentials, so it skips notarizing the app. Only the disk
    /// image is notarized, which also registers the app inside it with Apple.
    pub(crate) fn apply(&self, command: &mut Command) {
        command
            .env("APPLE_CERTIFICATE", &self.certificate)
            .env("APPLE_CERTIFICATE_PASSWORD", &self.certificate_password)
            // Overrides the ad-hoc identity in tauri.conf.json. Tauri checks that the
            // imported certificate's name contains it.
            .env("APPLE_SIGNING_IDENTITY", "Developer ID Application");
    }
}

/// App Store Connect API key used to notarize macOS bundles.
pub(crate) struct Notary {
    team_id: String,
    api_issuer: String,
    api_key_id: String,
    api_key: NamedTempFile,
}

impl Notary {
    /// Reads the API key from the release workflow secrets.
    pub(crate) fn from_env() -> Result<Self> {
        let team_id = required_env("APPLE_TEAM_ID")?;
        let api_issuer = required_env("APPLE_NOTARY_API_ISSUER_ID")?;
        let api_key_id = required_env("APPLE_NOTARY_API_KEY_ID")?;
        let api_key_pem = required_env("APPLE_NOTARY_API_KEY")?;
        if !api_key_pem
            .trim_start()
            .starts_with("-----BEGIN PRIVATE KEY-----")
        {
            bail!("APPLE_NOTARY_API_KEY must contain the .p8 private key in PEM format");
        }

        // notarytool only accepts the API key as a file.
        let mut api_key = tempfile::Builder::new()
            .suffix(".p8")
            .tempfile()
            .context("failed to create notary API key file")?;
        api_key
            .write_all(api_key_pem.as_bytes())
            .context("failed to write notary API key file")?;

        Ok(Self {
            team_id,
            api_issuer,
            api_key_id,
            api_key,
        })
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

    fn json(&self, args: &[&str], path: Option<&Path>) -> Result<Value> {
        let mut command = self.notarytool(args);
        command.args(["--output-format", "json"]);
        if let Some(path) = path {
            command.arg(path);
        }
        let output = command
            .stderr(Stdio::inherit())
            .output()
            .context("failed to run notarytool")?;
        require_success(output.status, &format!("notarytool {}", args[0]))?;
        serde_json::from_slice(&output.stdout).context("notarytool returned invalid JSON")
    }

    /// Uploads a file for notarization without waiting, and returns the submission ID.
    pub(crate) fn submit(&self, path: &Path) -> Result<String> {
        let submission = self.json(&["submit"], Some(path))?;
        let id = submission["id"]
            .as_str()
            .context("notarytool submit returned no submission ID")?;
        println!("notarization submission {id} for {}", path.display());
        Ok(id.to_owned())
    }

    /// Waits for a submission and fails unless Apple accepted it.
    pub(crate) fn wait(&self, id: &str) -> Result<()> {
        // notarytool does not document how `wait` reports its timeout, so ask for the
        // status separately.
        drop(
            self.notarytool(&["wait", id, "--timeout", NOTARIZATION_WAIT])
                .status(),
        );
        let info = self.json(&["info", id], None)?;
        match info["status"].as_str() {
            Some("Accepted") => Ok(()),
            Some("In Progress") => bail!(
                "Apple is still processing notarization {id}. Re-run this job to keep \
                 waiting for the same submission."
            ),
            status => {
                drop(self.notarytool(&["log", id]).status());
                bail!(
                    "notarization {id} finished with status {}. Re-running this job will \
                     not help: fix the bundle and build it again.",
                    status.unwrap_or("unknown")
                )
            }
        }
    }

    /// Verifies that the disk image and the app in the updater archive pass Gatekeeper
    /// as downloaded copies would. The app is not stapled: Gatekeeper finds its ticket,
    /// issued by the disk image's notarization, online.
    pub(crate) fn verify(&self, dmg: &Path, updater: &Path, app_name: &str) -> Result<()> {
        let team_id = &self.team_id;
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
        assess(&app, &["--type", "execute"])
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

/// Staples an accepted notarization ticket to a file.
pub(crate) fn staple(path: &Path) -> Result<()> {
    let status = Command::new("xcrun")
        .args(["stapler", "staple"])
        .arg(path)
        .status()
        .context("failed to run stapler")?;
    require_success(status, "stapler staple")
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
