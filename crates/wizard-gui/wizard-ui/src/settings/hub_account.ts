import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

type ProfileHub = { hub_id: string; hub_name: string; display_name: string | null; is_personal: boolean };
type HubProfile = { personal_hub_id: string; personal_hub_name: string; username: string; verified_email: string | null; hubs: ProfileHub[] };
type HubAccount = { signed_in: boolean; profile: HubProfile | null; active_hub_id: string | null; pending: boolean; error: string | null };
type HubAccountSnapshot = HubAccount & { revision: number };

export function useHubAccount() {
  const [account, setAccount] = useState<HubAccount>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: HubAccountSnapshot = await invoke<HubAccountSnapshot>("get_hub_account_state", { lastRevision });
          if (stopped) return;
          setAccount(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setAccount(undefined);
          setError("Не удалось получить состояние аккаунта");
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  return { account, error };
}

export const loginHubAccount = () => invoke<void>("login_hub_account");
export const logoutHubAccount = () => invoke<void>("logout_hub_account");
export const selectHub = (hubId: string) => invoke<void>("select_hub", { hubId });
export const openHub = (theme: string) => invoke<void>("open_hub", { theme });
