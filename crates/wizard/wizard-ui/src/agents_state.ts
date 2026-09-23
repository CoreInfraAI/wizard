import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { reportError } from "./log";

export type AgentDetection =
  | { status: "found"; data: { path: string; version: string | null } }
  | { status: "not_found" }
  | { status: "error"; data: string };

export type AgentStates = {
  codex_cli: AgentDetection;
  codex_desktop: AgentDetection;
};

type DetectionState =
  | { status: "loading" }
  | { status: "ready"; data: AgentStates }
  | { status: "error"; message: string };

// Share the initial request between React StrictMode effect invocations.
let initialRequest: Promise<AgentStates> | undefined;

export function useAgentState(): DetectionState {
  const [state, setState] = useState<DetectionState>({ status: "loading" });

  useEffect(() => {
    initialRequest ??= invoke<AgentStates>("get_agent_state");
    void initialRequest.then(
      (data) => setState({ status: "ready", data }),
      (cause: unknown) => {
        reportError("failed to detect agents", cause);
        setState({
          status: "error",
          message: cause instanceof Error ? cause.message : String(cause),
        });
      },
    );
  }, []);

  return state;
}
