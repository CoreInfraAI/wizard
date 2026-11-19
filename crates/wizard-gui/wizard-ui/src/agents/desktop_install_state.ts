import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

// Keep in sync with `DesktopApp` in `crates/wizard-core/src/agents/desktop_install/mod.rs`.
export type DesktopApp = "claude" | "chatgpt";

const names: Record<DesktopApp, { name: string }> = {
  claude: { name: "Claude" },
  chatgpt: { name: "ChatGPT" },
};

export type Operation = "check" | "download" | "install";
type Release = { version: string; path: string | null; notes: string[]; confirmation: string | null };
/** Bytes retained after a network interruption; explicit cancellation discards them. */
type Saved = { received: number; total: number };
export type Phase =
  | { status: "idle" | "checking" | "up_to_date" }
  // `close`: the user agreed to close the running applications, which open again afterwards.
  | { status: "installing"; close: boolean }
  | ({ status: "available"; saved: Saved | null } & Release)
  | ({ status: "ready_to_install" } & Release)
  // `total` is 0 until known.
  | { status: "downloading"; received: number; total: number; reconnecting: boolean }
  | { status: "installed"; version: string; notices: string[] }
  | { status: "unsupported"; message: string }
  | { status: "failed"; operation: Operation; message: string; confirmation: string | null };
export type Snapshot = Phase & { revision: number };

export const megabytes = (bytes: number) => Math.floor(bytes / (1024 * 1024)).toLocaleString("ru-RU");
const RUNNING_POLL_MS = 2000;

/** Products that installation would close because they run now, polled while `active`. */
export function useRunningApps(desktop: DesktopApp, active: boolean) {
  const [running, setRunning] = useState<string[]>([]);
  const [checked, setChecked] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => {
    setRunning([]);
    setChecked(false);
    setError(undefined);
    if (!active) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const next = await invoke<string[]>("get_desktop_running_apps", { desktop });
        if (stopped) return;
        setRunning((current) => current.join("\n") === next.join("\n") ? current : next);
        setChecked(true);
        setError(undefined);
      } catch {
        if (stopped) return;
        setChecked(false);
        setError("Не удалось проверить запущенные приложения. Повторяем проверку…");
      }
      if (!stopped) timer = setTimeout(() => void poll(), RUNNING_POLL_MS);
    }
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [desktop, active]);
  return { running, checked, error };
}

export function useDesktopInstall(desktop: DesktopApp) {
  const [state, setState] = useState<Snapshot>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: Snapshot = await invoke<Snapshot>("get_desktop_install_state", { desktop, lastRevision });
          if (stopped) return;
          setState(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setError(`Не удалось получить состояние установки ${names[desktop].name}`);
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, [desktop]);

  const busy = state?.status === "checking" || state?.status === "downloading" || state?.status === "installing";
  // Hidden until the state loads, so that an unsupported application never flashes.
  const hidden = state === undefined ? error === undefined : state.status === "unsupported";
  return { desktop, state, error, busy, hidden };
}
