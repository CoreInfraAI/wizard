import { type ReactNode, useState } from "react";
import { type AgentDetection, type AgentStates, sendAgentEvent } from "./agents_state";
import { reportError } from "./log";

function CodexProxy({ installed }: { installed: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send() {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(installed ? "CodexUninstall" : "CodexInstall");
    } catch (cause: unknown) {
      reportError("failed to send Codex proxy event", cause);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <div style={{ display: "flex", alignItems: "center", gap: "1rem" }}>
        <span>proxy: {installed ? "установлен" : "нет"}</span>
        <button type="button" disabled={pending} onClick={() => void send()}>
          {installed ? "удалить" : "установить"}
        </button>
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
      <Installation name="Codex" detection={agents.codex}>
        {agents.codex.status === "found" && (
          <CodexProxy installed={agents.codex.data.proxy_installed} />
        )}
      </Installation>
      <Installation name="ChatGPT" detection={agents.chatgpt} />
    </>
  );
}
