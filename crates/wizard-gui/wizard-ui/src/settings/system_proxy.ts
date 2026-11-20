import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";

type SystemProxyState = {
  status: "unknown" | "off" | "on" | "partial" | "unavailable" | "unsupported";
  busy: boolean;
  conflict: boolean;
  listener_running: boolean;
  read_error: string | null;
  operation_error: string | null;
};
type SystemProxySnapshot = SystemProxyState & { revision: number };

// Observes OS state without automatically changing it.
// Tray actions remain visible after reopening the window.
export function useSystemProxy(active = true) {
  const [state, setState] = useState<SystemProxyState>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!active) return;
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
  }, [active]);

  return { state, error };
}

export function setSystemProxy(enabled: boolean, forced = false): Promise<void> {
  return invoke<void>("set_system_proxy", { enabled, forced });
}

// Both prompts use the ordinary Settings action, which respects existing proxies.
export function useEnableSystemProxy() {
  const inFlight = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string>();

  async function enable() {
    if (inFlight.current) return false;
    inFlight.current = true;
    setSubmitting(true);
    setError(undefined);
    try {
      await setSystemProxy(true);
      return true;
    } catch (cause) {
      setError(
        typeof cause === "string"
          ? cause
          : "Не удалось включить системный прокси. Попробуйте ещё раз.",
      );
      return false;
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }

  return { enable, submitting, error };
}
