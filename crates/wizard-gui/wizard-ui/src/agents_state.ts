import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError, info, debug } from "./log";

export type AgentEvent =
  | "CodexInstall"
  | "CodexUninstall"
  | { SetCoreinfraToken: string };

export function sendAgentEvent(event: AgentEvent): Promise<void> {
  const name = typeof event === "string" ? event : "SetCoreinfraToken";
  info(`sending agent event: ${name}`);
  return invoke<void>("agent_event", { event });
}

export type AgentDetection<T> =
  | { status: "found"; data: T }
  | { status: "not_found" }
  | { status: "error"; data: string };

export type Codex = {
  path: string;
  version: string;
  proxy_installed: boolean;
};

export type ChatGpt = {
  path: string;
  version: string | null;
};

export type AgentStates = {
  codex: AgentDetection<Codex>;
  chatgpt: AgentDetection<ChatGpt>;
};

type DetectionState =
  | { status: "loading" }
  | { status: "ready"; data: AgentStateSnapshot }
  | { status: "error"; message: string };

type AgentStateSnapshot = {
  revision: string;
  agents: AgentStates;
  coreinfra_token_set: boolean;
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
          debug(`received agent state at revision ${snapshot.revision}`);
          setState({ status: "ready", data: snapshot });
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
