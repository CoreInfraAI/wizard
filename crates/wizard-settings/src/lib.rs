//! Backend-only settings storage. Secrets (Hub session, proxy tokens) are stored as plaintext,
//! never logged.

pub mod paths;

use anyhow::{Context as _, Result, anyhow, bail};
use paths::settings_path;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

// No Debug: settings contain a secret and must not be logged.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// Local proxy demand, independent of the shared upstream credential.
    pub claude_proxy_enabled: bool,
    pub codex_proxy_enabled: bool,
    pub system_proxy_enabled: bool,
    /// The coreinfra-ai-wizard token, shared by both listeners. Never log it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_token: Option<String>,
    /// Account session, independent of agent tokens. Never expose through IPC.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hub_session: String,
    /// Selected Hub for listing and creating agent tokens; empty uses the personal hub.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_hub_id: String,
    /// Random id that tells Wizard installations apart in Hub update metrics. Not a secret.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub install_id: String,
    /// Saved as `[video]` up to 1.0.10; loaded in memory and written as `[media]` on save.
    #[serde(alias = "video")]
    pub media: MediaSettings,
    /// Application lifecycle preferences, independent of proxy demand.
    pub application: ApplicationSettings,
    /// Kept last: it serializes as a TOML table, written after plain keys.
    pub last_paths: LastPaths,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaAgent {
    Codex,
    Claude,
    ClaudeDesktop,
}

/// Local MCP grants only; never serialize upstream API keys.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MediaSettings {
    pub codex: Option<String>,
    pub claude: Option<String>,
    pub claude_desktop: Option<String>,
    /// Agents whose media the user turned off. Others are offered media by default.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub declined: Vec<MediaAgent>,
}

impl MediaSettings {
    #[must_use]
    pub fn bearer(&self, agent: MediaAgent) -> Option<&str> {
        match agent {
            MediaAgent::Codex => &self.codex,
            MediaAgent::Claude => &self.claude,
            MediaAgent::ClaudeDesktop => &self.claude_desktop,
        }
        .as_deref()
    }
    #[must_use]
    pub fn has_grants(&self) -> bool {
        self.codex.is_some() || self.claude.is_some() || self.claude_desktop.is_some()
    }
    /// Whether Wizard offers media for `agent`: not granted and never turned off by the user.
    #[must_use]
    pub fn suggested(&self, agent: MediaAgent) -> bool {
        self.bearer(agent).is_none() && !self.declined.contains(&agent)
    }
    /// A grant clears the agent's opt-out; removing one records it.
    pub fn set(&mut self, agent: MediaAgent, bearer: Option<String>) {
        self.declined.retain(|declined| *declined != agent);
        if bearer.is_none() {
            self.declined.push(agent);
        }
        *match agent {
            MediaAgent::Codex => &mut self.codex,
            MediaAgent::Claude => &mut self.claude,
            MediaAgent::ClaudeDesktop => &mut self.claude_desktop,
        } = bearer;
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ApplicationSettings {
    pub tray_enabled: bool,
    pub autostart_enabled: bool,
}

impl Default for ApplicationSettings {
    fn default() -> Self {
        Self {
            tray_enabled: true,
            autostart_enabled: true,
        }
    }
}

/// Last found agent paths: first search candidates, not cached detection results.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LastPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_desktop: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_desktop: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi: Option<PathBuf>,
}

impl Settings {
    #[must_use]
    pub fn daemon_needed(&self) -> bool {
        self.local_proxy_enabled() || self.media.has_grants()
    }

    #[must_use]
    pub fn local_proxy_enabled(&self) -> bool {
        self.claude_proxy_enabled || self.codex_proxy_enabled || self.system_proxy_enabled
    }
}

// Lowercase RFC 4648 base32; Hub accepts exactly this format.
const INSTALL_ID_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
const INSTALL_ID_LEN: usize = 10;

/// 50 random bits as ten base32 characters.
#[must_use]
pub fn new_install_id() -> String {
    // A v4 UUID keeps its version and variant bits above bit 61; the low 50 bits are random.
    let bits = uuid::Uuid::new_v4().as_u128();
    (0..INSTALL_ID_LEN)
        .map(|index| {
            let digit = (bits >> (5 * index)) & 0x1f;
            char::from(INSTALL_ID_ALPHABET[usize::try_from(digit).expect("5-bit digit")])
        })
        .collect()
}

#[must_use]
pub fn is_install_id(value: &str) -> bool {
    value.len() == INSTALL_ID_LEN
        && value
            .bytes()
            .all(|byte| INSTALL_ID_ALPHABET.contains(&byte))
}

/// Convert legacy per-app token strings to demand flags before GUI initialization.
/// Reading settings stays strict. Never acquire credentials or start a service here.
/// Preserve unrelated fields, including settings unknown to this version.
pub fn migrate() -> Result<()> {
    let path = settings_path()?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to read Wizard settings for migration"),
    };
    if let Some(serialized) = migrate_text(&text)? {
        save_text(&path, &serialized)?;
    }
    Ok(())
}

/// The migrated document, or `None` when it needs no changes.
fn migrate_text(text: &str) -> Result<Option<String>> {
    // Parser diagnostics may contain credentials; expose only static messages.
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|_| anyhow!("failed to parse Wizard settings for migration"))?;
    let mut changed = false;
    for key in ["claude_proxy_enabled", "codex_proxy_enabled"] {
        if let Some(item) = document.get_mut(key)
            && let Some(token) = item.as_str()
        {
            let enabled = !token.is_empty();
            *item = toml_edit::value(enabled);
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    let serialized = document.to_string();
    // Do not partially migrate a file with other invalid settings.
    let _: Settings = toml_edit::de::from_str(&serialized)
        .map_err(|_| anyhow!("failed to validate migrated Wizard settings"))?;
    Ok(Some(serialized))
}

pub fn load_from_file() -> Result<Settings> {
    let text = match fs::read_to_string(settings_path()?) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => return Err(error).context("failed to read Wizard settings"),
    };
    // Do not retain the parser error: it may contain the API key.
    toml_edit::de::from_str(&text).map_err(|_| anyhow!("failed to deserialize Wizard settings"))
}

pub fn save_to_file(settings: &Settings) -> Result<()> {
    let path = settings_path()?;
    let serialized = toml_edit::ser::to_string_pretty(settings)
        .map_err(|_| anyhow!("failed to serialize Wizard settings"))?;
    save_text(&path, &serialized)
}

// Shared atomic writer for normal saves and format-preserving migrations.
fn save_text(path: &Path, serialized: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("Wizard settings file is a symlink; refusing to replace it");
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to inspect Wizard settings file"),
    }
    let parent = path.parent().context("settings path has no parent")?;
    fs::create_dir_all(parent).context("failed to create Wizard settings directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .context("failed to create temporary Wizard settings file")?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to restrict Wizard settings permissions")?;
    }
    file.write_all(serialized.as_bytes())
        .context("failed to write Wizard settings")?;
    file.as_file()
        .sync_all()
        .context("failed to sync Wizard settings")?;
    file.persist(path)
        .context("failed to save Wizard settings")?;
    Ok(())
}

pub fn update_file(edit: impl FnOnce(&mut Settings)) -> Result<()> {
    let current = load_from_file()?;
    let mut settings = current.clone();
    edit(&mut settings);
    if settings == current {
        return Ok(());
    }
    save_to_file(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_grants_round_trip() -> Result<()> {
        let mut s = Settings::default();
        s.media.set(MediaAgent::Claude, Some("local".into()));
        s.media.set(MediaAgent::ClaudeDesktop, None);
        let restored: Settings = toml_edit::de::from_str(&toml_edit::ser::to_string_pretty(&s)?)?;
        assert!(restored.media == s.media);
        Ok(())
    }

    #[test]
    fn media_is_suggested_until_the_user_turns_it_off() {
        let mut media = MediaSettings::default();
        assert!(media.suggested(MediaAgent::Codex));
        media.set(MediaAgent::Codex, Some("local".into()));
        assert!(!media.suggested(MediaAgent::Codex));
        media.set(MediaAgent::Codex, None);
        assert!(!media.suggested(MediaAgent::Codex));
        assert!(!media.has_grants());
        assert!(media.suggested(MediaAgent::Claude));
        media.set(MediaAgent::Codex, Some("again".into()));
        assert!(media.declined.is_empty());
    }

    #[test]
    fn legacy_video_table_carries_grants_and_opt_outs() -> Result<()> {
        let legacy = "install_id = 'abcdefgh27'\n\n[video]\nclaude = 'wizard-video-grant'\ndeclined = ['codex']\n";
        let read: Settings = toml_edit::de::from_str(legacy)?;
        assert_eq!(read.media.claude.as_deref(), Some("wizard-video-grant"));
        assert_eq!(read.media.declined.len(), 1);
        assert!(!read.media.suggested(MediaAgent::Codex));
        // Startup migration must not force a write (e.g. on a symlinked 1.0.10 settings file).
        assert!(migrate_text(legacy)?.is_none());
        let saved = toml_edit::ser::to_string_pretty(&read)?;
        assert!(!saved.contains("[video]") && saved.contains("[media]"));
        let restored: Settings = toml_edit::de::from_str(&saved)?;
        assert!(restored == read);
        Ok(())
    }

    #[test]
    fn application_defaults_and_partial_tables() {
        for text in ["", "[last_paths]\n", "[application]\n"] {
            let settings: Settings = toml_edit::de::from_str(text).expect("valid settings");
            assert!(settings.application.tray_enabled);
            assert!(settings.application.autostart_enabled);
        }
        let settings: Settings = toml_edit::de::from_str("[application]\ntray_enabled = false\n")
            .expect("partial application settings");
        assert!(!settings.application.tray_enabled);
        assert!(settings.application.autostart_enabled);
        assert!(Settings::default().application.tray_enabled);
        assert!(Settings::default().application.autostart_enabled);
    }

    #[test]
    fn application_preferences_round_trip_independently_of_proxy_demand() {
        for tray_enabled in [false, true] {
            for autostart_enabled in [false, true] {
                let settings = Settings {
                    application: ApplicationSettings {
                        tray_enabled,
                        autostart_enabled,
                    },
                    claude_proxy_enabled: true,
                    proxy_token: Some("fixture".into()),
                    ..Settings::default()
                };
                let text = toml_edit::ser::to_string_pretty(&settings).expect("serialize settings");
                let restored: Settings = toml_edit::de::from_str(&text).expect("read settings");
                assert_eq!(restored.application.tray_enabled, tray_enabled);
                assert_eq!(restored.application.autostart_enabled, autostart_enabled);
                assert!(restored.local_proxy_enabled());
            }
        }
    }

    #[test]
    fn new_install_ids_match_hub_format() {
        let first = new_install_id();
        assert!(is_install_id(&first), "{first:?}");
        assert_ne!(first, new_install_id());
        for invalid in ["", "abcdefgh2", "abcdefgh277", "ABCDEFGH27", "abcdefgh18"] {
            assert!(!is_install_id(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn install_id_round_trips() {
        let settings = Settings {
            install_id: "abcdefgh27".into(),
            ..Settings::default()
        };
        let text = toml_edit::ser::to_string_pretty(&settings).expect("serialize settings");
        let restored: Settings = toml_edit::de::from_str(&text).expect("read settings");
        assert_eq!(restored.install_id, "abcdefgh27");
    }

    #[test]
    fn existing_settings_default_to_personal_hub() {
        let settings: Settings = toml_edit::de::from_str("hub_session = 'session'\n[last_paths]\n")
            .expect("settings without a selected hub should deserialize");
        assert!(settings.active_hub_id.is_empty());
    }

    #[test]
    fn selected_hub_round_trips_with_discovered_paths() {
        let settings = Settings {
            active_hub_id: "team".into(),
            last_paths: LastPaths {
                codex: Some(PathBuf::from("/usr/local/bin/codex")),
                ..LastPaths::default()
            },
            ..Settings::default()
        };
        let text = toml_edit::ser::to_string_pretty(&settings)
            .expect("settings with a selected hub and paths should serialize");
        let restored: Settings =
            toml_edit::de::from_str(&text).expect("serialized settings should deserialize");
        assert_eq!(restored.active_hub_id, "team");
        assert_eq!(restored.last_paths.codex, settings.last_paths.codex);
    }
}
