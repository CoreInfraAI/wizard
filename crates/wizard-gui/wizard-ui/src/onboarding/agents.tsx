import type { ComponentType, SVGProps } from "react";
import { ClaudeIcon, OpenAiIcon, OpenCodeIcon, PiIcon } from "../components/BrandIcons";
import type { AgentId, AgentStates } from "../agents/agents_state";
import type { DesktopApp } from "../agents/DesktopInstall";

export type ChoiceId = AgentId | "chatgpt";
export type AgentChoice = {
  id: ChoiceId;
  agent: AgentId;
  name: string;
  Icon: ComponentType<SVGProps<SVGSVGElement>>;
  desktop?: DesktopApp;
};

export const choices: AgentChoice[] = [
  { id: "claude_desktop", agent: "claude_desktop", name: "Claude Desktop", Icon: ClaudeIcon, desktop: "claude" },
  { id: "chatgpt", agent: "codex", name: "ChatGPT", Icon: OpenAiIcon, desktop: "chatgpt" },
  { id: "claude", agent: "claude", name: "Claude Code", Icon: ClaudeIcon },
  { id: "codex", agent: "codex", name: "Codex CLI", Icon: OpenAiIcon },
  { id: "opencode", agent: "opencode", name: "OpenCode", Icon: OpenCodeIcon },
  { id: "pi", agent: "pi", name: "Pi", Icon: PiIcon },
];

export function isFound(choice: AgentChoice, agents: AgentStates) {
  if (choice.agent === "codex") {
    return agents.codex.status === "found" && agents.codex.data[choice.id === "chatgpt" ? "desktop" : "cli"].status === "found";
  }
  return agents[choice.agent].status === "found";
}

export function AgentIcon({ choice, className = "" }: { choice: AgentChoice; className?: string }) {
  return <choice.Icon className={`size-6 shrink-0 text-zinc-700 dark:text-zinc-300 ${className}`} aria-hidden="true" />;
}
