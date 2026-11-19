import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError } from "../logs/log";

let applicationVersionRequest: Promise<string> | undefined;

export type UpdateState =
  | { status: "checking" }
  | { status: "available"; version: string }
  | { status: "downloading"; version: string; received: number; total: number | null }
  | { status: "ready_to_install"; version: string }
  | { status: "installing"; version: string }
  | { status: "up_to_date" }
  | { status: "failed"; message: string; version?: string | null };

export type UpdateObservation = {
  state: UpdateState | undefined;
  error: string | undefined;
};

type UpdateSnapshot = UpdateState & { revision: number };

export function updateProgress(state: UpdateState | undefined) {
  return state?.status === "downloading" && state.total && state.total > 0
    ? Math.min(100, Math.floor(state.received / state.total * 100)) : undefined;
}

export function useUpdateState(): UpdateObservation {
  const [state, setState] = useState<UpdateState>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;

    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: UpdateSnapshot = await invoke<UpdateSnapshot>("get_update_state", { lastRevision });
          if (stopped) return;
          setState(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch (cause: unknown) {
          if (stopped) return;
          const message = "Не удалось получить состояние обновления";
          reportError(message, cause);
          setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
          // Read the current snapshot on retry, even if its revision has not changed.
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000)); // 1 sec
        }
      }
    }

    void observe();
    return () => {
      // An outstanding IPC wait completes on the next state change.
      stopped = true;
    };
  }, []);

  return { state, error };
}

export function useApplicationVersion() {
  const [applicationVersion, setApplicationVersion] = useState<string>();

  useEffect(() => {
    applicationVersionRequest ??= getVersion();
    void applicationVersionRequest
      .then(setApplicationVersion)
      .catch((error: unknown) => {
        reportError("failed to get application version", error);
      });
  }, []);

  return applicationVersion;
}
