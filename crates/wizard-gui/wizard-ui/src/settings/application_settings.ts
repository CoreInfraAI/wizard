import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export type ApplicationSetting = "tray_enabled" | "autostart_enabled";
type ApplicationSettings = {
  tray_enabled: boolean;
  autostart_enabled: boolean;
  supported: boolean;
};
type ApplicationSettingsSnapshot = ApplicationSettings & { revision: number };

// The backend owns saved preferences; this hook only observes its safe IPC projection.
export function useApplicationSettings() {
  const [settings, setSettings] = useState<ApplicationSettings>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: ApplicationSettingsSnapshot = await invoke<ApplicationSettingsSnapshot>("get_application_settings", { lastRevision });
          if (stopped) return;
          setSettings(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setError("Не удалось получить настройки приложения");
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  return { settings, error };
}

export function setApplicationSetting(setting: ApplicationSetting, enabled: boolean): Promise<void> {
  return invoke<void>("set_application_setting", { setting, enabled });
}
