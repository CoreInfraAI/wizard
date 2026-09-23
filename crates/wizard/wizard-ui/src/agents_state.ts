import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError, info } from "./log";

export type AgentEvent = "CodexCliInstall" | "CodexCliUninstall";

export function sendAgentEvent(event: AgentEvent): Promise<void> {
  return invoke<void>("agent_event", { event });
}

export type AgentDetection<T> =
  | { status: "found"; data: T }
  | { status: "not_found" }
  | { status: "error"; data: string };

export type CodexCli = {
  path: string;
  version: string;
  proxy_installed: boolean;
};

export type CodexDesktop = {
  path: string;
  version: string | null;
};

export type AgentStates = {
  codex_cli: AgentDetection<CodexCli>;
  codex_desktop: AgentDetection<CodexDesktop>;
};

type DetectionState =
  | { status: "loading" }
  | { status: "ready"; data: AgentStates }
  | { status: "error"; message: string };

type AgentStateSnapshot = {
  revision: string;
  agents: AgentStates;
};

// Deduplicate concurrent reads, but never cache a completed snapshot.
let pendingSnapshot: Promise<AgentStateSnapshot> | undefined;

function getSnapshot(): Promise<AgentStateSnapshot> {
  pendingSnapshot ??= invoke<AgentStateSnapshot>("get_agent_state").finally(() => {
    pendingSnapshot = undefined;
  });
  return pendingSnapshot;
}

export function useAgentState(): DetectionState {
  const [state, setState] = useState<DetectionState>({ status: "loading" });

  useEffect(() => {
    let stopped = false;

    async function observe() {
      try {
        while (!stopped) {
          const snapshot = await getSnapshot();
          if (stopped) return;
          info("got AgentState")
          setState({ status: "ready", data: snapshot.agents });
          await invoke<string>("wait_for_update", { lastRevision: snapshot.revision });
        }
      } catch (cause: unknown) {
        if (stopped) return;
        reportError("failed to observe agent state", cause);
        setState({
          status: "error",
          message: cause instanceof Error ? cause.message : String(cause),
        });
      }
    }

    void observe();
    return () => {
      // Stop the loop after unmount; an outstanding IPC wait cannot be cancelled here.
      stopped = true;
    };
  }, []);

  return state;
}
