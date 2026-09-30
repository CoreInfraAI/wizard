import { type ReactNode, useState } from "react";
import { type AgentDetection, type AgentStates, type CodexProxyMode, sendAgentEvent } from "./agents_state";
import { reportError } from "./log";

const codexProxyOptions = [
  { mode: "disabled", label: "Выключен" },
  { mode: "proxy_hub", label: "CoreInfra Hub" },
  { mode: "proxy_api", label: "CoreInfra API · ChatGPT" },
] as const;

function CodexProxy({ mode }: { mode: CodexProxyMode }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send(proxyMode: CodexProxyMode) {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent({ CodexSetProxy: proxyMode });
    } catch (cause: unknown) {
      reportError("failed to send Codex proxy event", cause);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <p>Прокси</p>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
        {codexProxyOptions.map((option) => (
          <button
            key={option.mode}
            type="button"
            disabled={pending}
            style={{ fontWeight: mode === option.mode ? "bold" : "normal" }}
            onClick={() => void send(option.mode)}
          >
            {option.label}
          </button>
        ))}
      </div>
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
            <CodexProxy mode={agents.codex.data.proxy_mode} />
          )}
        </Installation>
        <Installation name="ChatGPT" detection={agents.chatgpt} />
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))" }}>
        <Installation name="Claude Code" detection={agents.claude} />
        <Installation name="Claude Desktop" detection={agents.claude_desktop} />
      </div>
      <Installation name="OpenCode" detection={agents.opencode} />
      <Installation name="Pi" detection={agents.pi} />
    </>
  );
}
