import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError } from "../logs/log";

export type Proxy = { enabled: boolean };
type ProxySnapshot = Proxy & { revision: number };

export function useProxy(): { proxy: Proxy | undefined; error: string | undefined } {
  const [proxy, setProxy] = useState<Proxy>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;

    async function observe() {
      let lastRevision: number | null = null;
      let reported: string | undefined;
      while (!stopped) {
        try {
          const next: ProxySnapshot = await invoke<ProxySnapshot>("get_proxy_state", { lastRevision });
          if (stopped) return;
          setProxy(next);
          setError(undefined);
          reported = undefined;
          lastRevision = next.revision;
        } catch (cause: unknown) {
          if (stopped) return;
          const message = "Не удалось получить состояние прокси";
          const detail = `${message}: ${cause instanceof Error ? cause.message : String(cause)}`;
          // Retried every second: log a new error once, not every attempt.
          if (detail !== reported) reportError(message, cause);
          reported = detail;
          setProxy(undefined);
          setError(detail);
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

  return { proxy, error };
}
