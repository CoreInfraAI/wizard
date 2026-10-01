import { type ReactNode, useState } from "react";
import { type AgentDetection, type AgentStates, type ProxyMode, sendAgentEvent } from "./agents_state";
import { reportError } from "./log";

const proxyOptions = [
  { mode: "disabled", label: "Выключен" },
  { mode: "proxy_hub", label: "CoreInfra Hub" },
  { mode: "proxy_api", label: "CoreInfra API" },
] as const;

function AgentProxy({ agent, mode }: { agent: "codex" | "claude"; mode: ProxyMode }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send(proxyMode: ProxyMode) {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(agent === "codex"
        ? { CodexSetProxy: proxyMode }
        : { ClaudeSetProxy: proxyMode });
    } catch (cause: unknown) {
      reportError(`failed to send ${agent} proxy event`, cause);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <p>Прокси</p>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
        {proxyOptions.map((option) => (
          <button
            key={option.mode}
            type="button"
            disabled={pending}
            style={{ fontWeight: mode === option.mode ? "bold" : "normal" }}
            onClick={() => void send(option.mode)}
          >
            {option.label}{agent === "codex" && option.mode === "proxy_api" ? " · ChatGPT" : ""}
          </button>
        ))}
      </div>
      {error && <p>Не удалось отправить событие: {error}</p>}
    </>
  );
}

function HubProxy({ agent, installed }: { agent: "Pi" | "OpenCode"; installed: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send() {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(agent === "Pi"
        ? { SetPiHub: !installed }
        : { SetOpenCodeHub: !installed });
    } catch (cause: unknown) {
      reportError(`failed to send ${agent} Hub event`, cause);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <p>Прокси: {installed ? "установлен" : "нет"}</p>
      <button type="button" disabled={pending} onClick={() => void send()}>
        {installed ? "Удалить" : "Установить"}
      </button>
      {error && <p>Не удалось отправить событие: {error}</p>}
    </>
  );
}

function Installation({ name, detection, children }: {
  name: string;
  detection: AgentDetection<{ path: string; version: string | null }>;
  children?: ReactNode;
}) {
  return (
    <section>
      <h2>{name}</h2>
      {detection.status === "found" && (
        <>
          <dl>
            <dt>Version</dt>
            <dd>{detection.data.version ?? "Unknown"}</dd>
            <dt>Path</dt>
            <dd>{detection.data.path}</dd>
          </dl>
          {children}
        </>
      )}
      {detection.status === "not_found" && <p>Not found</p>}
      {detection.status === "error" && (
        <p>Detection failed: {detection.data}</p>
      )}
    </section>
  );
}

export function AgentList({ agents }: { agents: AgentStates }) {
  return (
    <>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))" }}>
        <Installation name="Codex" detection={agents.codex}>
          {agents.codex.status === "found" && (
            <AgentProxy agent="codex" mode={agents.codex.data.proxy_mode} />
          )}
        </Installation>
        <Installation name="ChatGPT" detection={agents.chatgpt} />
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))" }}>
        <Installation name="Claude Code" detection={agents.claude}>
          {agents.claude.status === "found" && (
            <AgentProxy agent="claude" mode={agents.claude.data.proxy_mode} />
          )}
        </Installation>
        <Installation name="Claude Desktop" detection={agents.claude_desktop} />
      </div>
      <Installation name="OpenCode" detection={agents.opencode}>
        {agents.opencode.status === "found" && (
          <HubProxy agent="OpenCode" installed={agents.opencode.data.proxy_installed} />
        )}
      </Installation>
      <Installation name="Pi" detection={agents.pi}>
        {agents.pi.status === "found" && (
          <HubProxy agent="Pi" installed={agents.pi.data.proxy_installed} />
        )}
      </Installation>
    </>
  );
}
