import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError, debug } from "../logs/log";

export type AgentChange =
  | { CodexSetRouting: RoutingMode }
  | { ClaudeSetRouting: RoutingMode }
  | { ClaudeDesktopSetRouting: RoutingMode }
  | { ClaudeDesktopSetMedia: boolean }
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
  // Resolved credential for re-preparation, never a create request. Do not log or persist it.
  token: string | null;
  plan: PreparedPlan;
  info: PreparedStepInfo[];
  restarts_desktop: boolean;
};

// Required for enabled modes except Claude Desktop subscription (shared Wizard proxy token).
// `create` issues a Hub token before preparing; it stays in Hub if the plan is cancelled.
// Values are secrets.
export type TokenChoice = { kind: "existing"; token: string } | { kind: "create" };

export type ConnectionDraft = {
  agent: AgentId;
  routing?: { change: AgentChange; token: TokenChoice | null };
  mediaEnabled?: boolean;
};

export type ConnectionPlan = {
  agent: AgentId;
  routing?: PreparedAgentChange;
  mediaEnabled?: boolean;
};

export async function prepareAgentChange({ agent, routing, mediaEnabled }: ConnectionDraft): Promise<ConnectionPlan> {
  const request = routing ?? (agent === "claude_desktop" && mediaEnabled !== undefined
    ? { change: { ClaudeDesktopSetMedia: mediaEnabled }, token: null }
    : undefined);
  const prepared = request
    ? await invoke<PreparedAgentChange>("prepare_agent_change", { ...request, media: mediaEnabled })
    : undefined;
  return { agent, routing: prepared, mediaEnabled };
}

export function applyAgentPlan(plan: PreparedPlan): Promise<void> {
  return invoke<void>("apply_agent_plan", { plan });
}

export function applyMediaConnection(agent: AgentId, enabled: boolean): Promise<void> {
  return invoke<void>("set_media_connection", { agent, enabled });
}

export type BackupAgent = "codex" | "claude" | "claude_desktop";

export type Backup = {
  id: number;
  time_created: string;
  files: Record<string, { content: string; mode?: string } | null>;
};

export function getAgentBackups(agent: BackupAgent): Promise<Backup[]> {
  // The response contains credentials; do not log or persist it in frontend storage.
  return invoke<Backup[]>("get_agent_backups", { agent });
}

export type DirectChange =
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
  proxy_needs_apply: boolean;
  // Legacy `coreinfra_video` registration or missing media skills from an older Wizard.
  media_needs_apply: boolean;
  // CoreInfra token from the agent's configuration. Secret: never log or persist it.
  token: string | null;
};

export type Claude = {
  media_needs_apply: boolean;
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
  configuration_needs_apply: boolean;
  media_needs_apply: boolean;
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

// Codex can be found through its CLI while desktop detection fails, or vice versa.
export function hasAgentDetectionError(detection: AgentStates[AgentId]) {
  return detection.status === "error" ||
    (detection.status === "found" && "desktop" in detection.data &&
      (detection.data.desktop.status === "error" || detection.data.cli.status === "error"));
}

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
      while (!stopped) {
        try {
          const snapshot = await getSnapshot();
          if (stopped) return;
          debug(`received agent state at revision ${snapshot.revision}`);
          setState({ status: "ready", data: snapshot });
          await invoke<number>("wait_for_update", { lastRevision: snapshot.revision });
        } catch (cause: unknown) {
          if (stopped) return;
          reportError("failed to observe agent state", cause);
          setState({
            status: "error",
            message: cause instanceof Error ? cause.message : String(cause),
          });
          await new Promise((resolve) => setTimeout(resolve, 1000)); // 1 sec
        }
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
