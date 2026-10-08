import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError } from "../logs/log";

export type Settings = { coreinfra_api_key?: string };
type SettingsSnapshot = Settings & { revision: number };

export function useSettings(): { settings: Settings | undefined; error: string | undefined } {
  const [settings, setSettings] = useState<Settings>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;

    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: SettingsSnapshot = await invoke<SettingsSnapshot>("get_settings_state", { lastRevision });
          if (stopped) return;
          setSettings(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch (cause: unknown) {
          if (stopped) return;
          const message = "Не удалось получить настройки";
          reportError(message, cause);
          setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }

    void observe();
    return () => {
      stopped = true;
    };
  }, []);

  return { settings, error };
}
