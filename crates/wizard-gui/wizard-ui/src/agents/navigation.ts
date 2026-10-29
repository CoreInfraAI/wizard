import type { AgentId } from "./agents_state";

export type AgentPageId = AgentId;
export type AppPageId = AgentPageId | "settings";
export const settingsPath = "/settings";

export const agentPaths: Record<AgentPageId, string> = {
  claude_desktop: "/agents/claude-desktop",
  claude: "/agents/claude",
  codex: "/agents/codex",
  opencode: "/agents/opencode",
  pi: "/agents/pi",
};

export function pageFromHash(hash: string): AppPageId {
  const path = hash.replace(/^#/, "").replace(/\/+$/, "");
  if (path === settingsPath) return "settings";
  return (Object.keys(agentPaths) as AgentPageId[])
    .find((page) => agentPaths[page] === path) ?? "claude_desktop";
}
