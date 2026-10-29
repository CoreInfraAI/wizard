import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

// `token` values are secrets: never log or persist them.
export type HubToken = { token: string; name: string; issued_at: number };
type HubTokens = { hub_id: string | null; tokens: HubToken[]; loading: boolean; error: string | null };
type HubTokensSnapshot = HubTokens & { revision: number };

// Only `ready` means the list is known; `tokens` keeps the last known list for labels.
export type HubTokensState =
  | { status: "loading"; tokens: HubToken[] }
  | { status: "error"; tokens: HubToken[]; error: string }
  | { status: "ready"; tokens: HubToken[] };

// Reloads the list after a failed load; the result arrives through `useHubTokens`.
export function refreshHubTokens(): Promise<void> {
  return invoke<void>("refresh_hub_tokens");
}

export function useHubTokens(activeHubId: string | null | undefined): HubTokensState {
  const [state, setState] = useState<HubTokens>();

  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: HubTokensSnapshot = await invoke<HubTokensSnapshot>("get_hub_tokens_state", { lastRevision });
          if (stopped) return;
          setState(next);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setState({ hub_id: null, loading: false, tokens: [], error: "Не удалось получить токены CoreInfra Hub" });
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  if (!state || (state.hub_id !== activeHubId && !(state.hub_id === null && state.error))) {
    return { status: "loading", tokens: [] };
  }
  if (state.loading) return { status: "loading", tokens: state.tokens };
  if (state.error !== null) return { status: "error", tokens: state.tokens, error: state.error };
  return { status: "ready", tokens: state.tokens };
}

// Shown instead of the whole secret value.
export function tokenPrefix(token: string): string {
  return `${token.slice(0, 12)}…`;
}

// Hub name of an agent's token, or its prefix when the name is unknown or empty.
export function tokenLabel(token: string, tokens: HubToken[]): string {
  const name = tokens.find((candidate) => candidate.token === token)?.name;
  return name ? name : tokenPrefix(token);
}

type ConfiguredTokenInfo = { name: string; hub_id: string };
type ConfiguredTokens = { details: (ConfiguredTokenInfo | null)[]; error: string | null };
type ConfiguredTokensSnapshot = ConfiguredTokens & { revision: number };

// Only metadata is returned. Keys stay in memory and are never logged or persisted here.
export function useConfiguredTokens(tokens: string[], accountId: string | undefined) {
  const [result, setResult] = useState<ConfiguredTokens & {
    tokens: string[];
    accountId: string;
  }>();

  useEffect(() => {
    if (!accountId || tokens.length === 0) return;
    let stopped = false;
    async function observe(currentAccountId: string) {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: ConfiguredTokensSnapshot = await invoke<ConfiguredTokensSnapshot>("get_configured_token_info", { tokens, lastRevision });
          if (stopped) return;
          setResult({ tokens, accountId: currentAccountId, ...next });
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setResult({ tokens, accountId: currentAccountId, details: [], error: "Не удалось получить сведения о ключах" });
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe(accountId);
    return () => { stopped = true; };
  }, [tokens, accountId]);

  const current = result?.tokens === tokens && result.accountId === accountId ? result : undefined;
  return {
    loading: Boolean(accountId && tokens.length > 0 && !current),
    error: current?.error,
    get: (token: string) => current?.details[tokens.indexOf(token)] ?? undefined,
  };
}
