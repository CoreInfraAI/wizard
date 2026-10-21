import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

// `token` values are secrets: never log or persist them.
export type HubToken = { token: string; name: string; issued_at: number };
type HubTokens = { tokens: HubToken[]; loading: boolean; error: string | null };
type HubTokensSnapshot = HubTokens & { revision: number };

// Only `ready` means the list is known; `tokens` keeps the last known list for labels.
export type HubTokensState =
  | { status: "loading"; tokens: HubToken[] }
  | { status: "error"; tokens: HubToken[]; error: string }
  | { status: "ready"; tokens: HubToken[] };

export function useHubTokens(): HubTokensState {
  const [state, setState] = useState<HubTokensState>({ status: "loading", tokens: [] });

  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: HubTokensSnapshot = await invoke<HubTokensSnapshot>("get_hub_tokens_state", { lastRevision });
          if (stopped) return;
          setState(next.loading ? { status: "loading", tokens: next.tokens }
            : next.error !== null ? { status: "error", tokens: next.tokens, error: next.error }
            : { status: "ready", tokens: next.tokens });
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setState({ status: "error", tokens: [], error: "Не удалось получить токены CoreInfra Hub" });
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  return state;
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
