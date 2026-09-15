import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

let applicationVersionRequest: Promise<string> | undefined;

export function useStartupUpdate() {
  const [isStartupUpdateComplete, setIsStartupUpdateComplete] = useState(false);

  useEffect(() => {
    async function runStartupUpdate() {
      try {
        await invoke<void>("wait_for_startup_update");
      } catch (error: unknown) {
        console.error("failed to wait for startup updater", error);
      }

      setIsStartupUpdateComplete(true);
    }

    void runStartupUpdate();
  }, []);

  return isStartupUpdateComplete;
}

export function useApplicationVersion() {
  const [applicationVersion, setApplicationVersion] = useState<string>();

  useEffect(() => {
    applicationVersionRequest ??= getVersion();
    void applicationVersionRequest
      .then(setApplicationVersion)
      .catch((error: unknown) => {
        console.error("failed to get application version", error);
      });
  }, []);

  return applicationVersion;
}
