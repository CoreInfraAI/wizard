use crate::revision_signal::{RevisionSignal, RevisionSnapshot};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};
use tauri::Manager as _;
use wizard_settings::{self as settings, ApplicationSettings, Settings};

static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) async fn initialize_state(app: &tauri::AppHandle) -> Result<()> {
    let settings = tauri::async_runtime::spawn_blocking(|| {
        // TODO: remove migration
        settings::migrate()?;
        settings::load_from_file()
    })
    .await
    .context("settings initialization task failed")??;

    if !app.manage(RevisionSignal::new(settings)) {
        bail!("settings already initialized");
    }
    Ok(())
}

/// TODO: maybe update state when file changes
pub(crate) fn get_state(app: &tauri::AppHandle) -> Result<Settings> {
    let signal = app
        .try_state::<RevisionSignal<Settings>>()
        .context("settings not initialized")?;
    Ok(signal.current().state)
}

pub(crate) async fn update_state(
    app: &tauri::AppHandle,
    edit: impl FnOnce(&mut Settings) + Send + 'static,
) -> Result<()> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = lock_state()?;
        update_state_locked(&app, &guard, edit)
    })
    .await
    .context("settings save task failed")?
}

/// Blocking callers can keep the account/Hub stable across an entire agent plan.
pub(crate) fn lock_state() -> Result<MutexGuard<'static, ()>> {
    SETTINGS_WRITE_LOCK
        .lock()
        .map_err(|_| anyhow!("settings write lock poisoned"))
}

/// Requires the guard from `lock_state`; never reacquires it from a proxy callback.
pub(crate) fn update_state_locked(
    app: &tauri::AppHandle,
    _guard: &MutexGuard<'_, ()>,
    edit: impl FnOnce(&mut Settings),
) -> Result<()> {
    let signal = app
        .try_state::<RevisionSignal<Settings>>()
        .context("settings not initialized")?;
    let current = signal.current().state;
    let mut next = current.clone();
    edit(&mut next);
    if next != current {
        settings::save_to_file(&next)
            .context("Не удалось сохранить настройки; изменения не применены")?;
        signal.notify(next);
    }

    Ok(())
}

fn sync_application_settings(app: &tauri::AppHandle, settings: ApplicationSettings) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let tray = crate::tray::set_enabled(app, settings.tray_enabled);
        let autostart = crate::autostart::set_enabled(app, settings.autostart_enabled);
        // Finish both effects before exiting if the last window closed while
        // disabling the tray. Explicit Exit remains unaffected.
        if !crate::tray::is_enabled(app) && app.webview_windows().is_empty() {
            app.exit(0);
        }
        tray.and(autostart)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (app, settings);
        bail!("Работа в трее и автозапуск пока доступны только на macOS и Windows")
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct ApplicationSettingsState {
    #[serde(flatten)]
    preferences: ApplicationSettings,
    supported: bool,
}

fn application_snapshot(
    snapshot: &RevisionSnapshot<Settings>,
) -> RevisionSnapshot<ApplicationSettingsState> {
    RevisionSnapshot {
        revision: snapshot.revision,
        state: ApplicationSettingsState {
            preferences: snapshot.state.application,
            supported: cfg!(any(target_os = "macos", target_os = "windows")),
        },
    }
}

/// Project only non-secret preferences from the single settings owner.
#[tauri::command]
pub(crate) async fn get_application_settings(
    last_revision: Option<u32>,
    signal: tauri::State<'_, RevisionSignal<Settings>>,
) -> Result<RevisionSnapshot<ApplicationSettingsState>, String> {
    signal
        .wait(last_revision)
        .await
        .map(|snapshot| application_snapshot(&snapshot))
        .map_err(|_| "Не удалось получить настройки приложения".into())
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApplicationSetting {
    TrayEnabled,
    AutostartEnabled,
}

fn set_preference(settings: &mut Settings, setting: ApplicationSetting, enabled: bool) {
    match setting {
        ApplicationSetting::TrayEnabled => settings.application.tray_enabled = enabled,
        ApplicationSetting::AutostartEnabled => settings.application.autostart_enabled = enabled,
    }
}

#[tauri::command]
pub(crate) async fn set_application_setting(
    app: tauri::AppHandle,
    setting: ApplicationSetting,
    enabled: bool,
) -> Result<(), String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        return Err("Работа в трее и автозапуск пока доступны только на macOS и Windows".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let guard = lock_state()?;
        update_state_locked(&app, &guard, |settings| {
            set_preference(settings, setting, enabled);
        })?;

        // Retry OS effects even when the saved preferences are unchanged.
        sync_application_settings(&app, get_state(&app)?.application)
            .context("Настройки сохранены, но не удалось применить их в системе")
    })
    .await
    .context("application settings task failed")
    .flatten()
    .map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_exposes_only_application_preferences() {
        let settings = Settings {
            hub_session: "fixture-session".into(),
            claude_proxy_enabled: true,
            proxy_token: Some("fixture-token".into()),
            ..Settings::default()
        };
        let snapshot = application_snapshot(&RevisionSnapshot {
            revision: 7,
            state: settings,
        });
        let json = serde_json::to_value(snapshot).expect("serialize safe snapshot");
        assert_eq!(json.as_object().expect("object").len(), 4);
        assert_eq!(json["revision"], 7);
        assert_eq!(json["tray_enabled"], true);
        assert_eq!(json["autostart_enabled"], true);
        assert!(json.get("supported").is_some());
    }

    #[test]
    fn preference_updates_preserve_other_settings() {
        let mut settings = Settings {
            hub_session: "fixture-session".into(),
            codex_proxy_enabled: true,
            proxy_token: Some("fixture-token".into()),
            ..Settings::default()
        };
        set_preference(&mut settings, ApplicationSetting::TrayEnabled, false);
        assert!(!settings.application.tray_enabled);
        assert!(settings.application.autostart_enabled);
        set_preference(&mut settings, ApplicationSetting::AutostartEnabled, false);
        assert!(!settings.application.tray_enabled);
        assert!(!settings.application.autostart_enabled);
        assert_eq!(settings.hub_session, "fixture-session");
        assert_eq!(settings.proxy_token.as_deref(), Some("fixture-token"));
    }
}
