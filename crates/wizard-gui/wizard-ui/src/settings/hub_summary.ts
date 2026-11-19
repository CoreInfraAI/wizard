import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export type HubSummary = {
  hub_id: string;
  role_in_hub: string;
  balance_rub: number | null;
};
type Observation = {
  hubId: string;
  data?: HubSummary;
  error?: string;
};

/** Refresh on return from the Hub and periodically while the overview is open. */
export function useHubSummary(hubId: string | null | undefined, signedIn: boolean) {
  const [observation, setObservation] = useState<Observation>();
  useEffect(() => {
    setObservation(undefined);
    if (!signedIn || !hubId) return;
    let stopped = false;
    let inFlight = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      if (stopped || inFlight) return;
      clearTimeout(timer);
      inFlight = true;
      try {
        const data = await invoke<HubSummary>("get_hub_summary", { hubId });
        if (data.hub_id !== hubId) throw new Error("Hub changed");
        if (!stopped) setObservation({ hubId: hubId!, data });
      } catch {
        // An unavailable balance is never treated as zero, including after a previous success.
        if (!stopped) setObservation({ hubId: hubId!, error: "Не удалось загрузить баланс" });
      } finally {
        inFlight = false;
        if (!stopped) timer = setTimeout(refresh, 30_000);
      }
    }
    const onVisible = () => { if (document.visibilityState === "visible") void refresh(); };
    void refresh();
    window.addEventListener("focus", onVisible);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      stopped = true;
      clearTimeout(timer);
      window.removeEventListener("focus", onVisible);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [hubId, signedIn]);
  const current = signedIn && observation?.hubId === hubId ? observation : undefined;
  return { data: current?.data, error: current?.error, loading: signedIn && !current };
}

export function needsHubTopUp(summary: HubSummary | undefined) {
  return !!summary && summary.role_in_hub !== "guest" &&
    summary.role_in_hub !== "" && typeof summary.balance_rub === "number" &&
    Number.isFinite(summary.balance_rub) && summary.balance_rub <= 0;
}

export const formatHubBalance = (balance: number) => new Intl.NumberFormat("ru-RU", {
  style: "currency", currency: "RUB", minimumFractionDigits: 0, maximumFractionDigits: 2,
}).format(balance);
