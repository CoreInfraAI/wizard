import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

type SystemProxyState = {
  status: "unknown" | "off" | "on" | "partial" | "unavailable" | "unsupported";
  busy: boolean;
  conflict: boolean;
  listener_running: boolean;
  read_error: string | null;
  operation_error: string | null;
};
type SystemProxySnapshot = SystemProxyState & { revision: number };

// Only observes the shared backend state. No saved flag, automatic mutation or
// UI-owned process state; tray actions remain visible after reopening the window.
export function useSystemProxy() {
  const [state, setState] = useState<SystemProxyState>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: SystemProxySnapshot = await invoke<SystemProxySnapshot>("get_system_proxy_state", { lastRevision });
          if (stopped) return;
          setState(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setError("Не удалось получить состояние системного прокси");
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  return { state, error };
}

export function setSystemProxy(enabled: boolean): Promise<void> {
  return invoke<void>("set_system_proxy", { enabled });
}
