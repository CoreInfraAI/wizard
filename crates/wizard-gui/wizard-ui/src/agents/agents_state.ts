import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError, debug } from "../logs/log";

export type AgentChange =
  | { CodexSetRouting: RoutingMode }
  | { ClaudeSetRouting: RoutingMode }
  | { ClaudeDesktopSetRouting: RoutingMode }
  | { SetPiHub: HubMode }
  | { SetOpenCodeHub: HubMode };

// Opaque backend payload: may contain credentials. Do not log, modify or persist it.
export type PreparedPlan = unknown;

export type PreparedStepInfo = {
  description: string;
};

export type AgentId = "codex" | "claude" | "claude_desktop" | "pi" | "opencode";

export type PreparedAgentChange = {
  change: AgentChange;
  agent: AgentId;
  plan: PreparedPlan;
  info: PreparedStepInfo[];
};

export function prepareAgentChange(change: AgentChange): Promise<PreparedAgentChange> {
  return invoke<PreparedAgentChange>("prepare_agent_change", { change });
}

export function applyAgentPlan(plan: PreparedPlan): Promise<void> {
  return invoke<void>("apply_agent_plan", { plan });
}

export type BackupAgent = "codex" | "claude";

export type Backup = {
  id: number;
  backup_version: number;
  time_created: string;
  files: Record<string, { content: string; mode?: string } | null>;
};

export function getAgentBackups(agent: BackupAgent): Promise<Backup[]> {
  // The response contains credentials; do not log or persist it in frontend storage.
  return invoke<Backup[]>("get_agent_backups", { agent });
}

export type DirectChange =
  | { SetCoreInfraToken: string }
  | { RestoreBackup: { agent: BackupAgent; id: number } };

export function applyDirectChange(change: DirectChange): Promise<void> {
  return invoke<void>("apply_direct_change", { change });
}

export type AgentDetection<T> =
  | { status: "found"; data: T }
  | { status: "not_found" }
  | { status: "error"; data: string };

export type RoutingMode = "disabled" | "sub" | "hub";
export type HubMode = "disabled" | "hub";

export type Codex = {
  cli: AgentDetection<{ path: string; version: string | null }>;
  desktop: AgentDetection<{ path: string; version: string | null }>;
  routing_mode: RoutingMode;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type Claude = {
  path: string;
  version: string;
  routing_mode: RoutingMode;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type ClaudeDesktop = {
  path: string;
  version: string | null;
  routing_mode: RoutingMode;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type OpenCode = {
  path: string;
  version: string;
  routing_mode: HubMode;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type Pi = {
  path: string;
  version: string;
  routing_mode: HubMode;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type AgentStates = {
  codex: AgentDetection<Codex>;
  claude: AgentDetection<Claude>;
  claude_desktop: AgentDetection<ClaudeDesktop>;
  opencode: AgentDetection<OpenCode>;
  pi: AgentDetection<Pi>;
};

type DetectionState =
  | { status: "loading" }
  | { status: "ready"; data: AgentStateSnapshot }
  | { status: "error"; message: string };

type AgentStateSnapshot = AgentStates & { revision: number };

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
          await invoke<number>("wait_for_update", { lastRevision: snapshot.revision });
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
