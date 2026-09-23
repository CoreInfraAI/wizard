import { type ReactNode, useState } from "react";
import { type AgentDetection, sendAgentEvent, useAgentState } from "./agents_state";
import { reportError } from "./log";

function CodexCliProxy({ installed }: { installed: boolean }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send() {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(installed ? "CodexCliUninstall" : "CodexCliInstall");
    } catch (cause: unknown) {
      reportError("failed to send Codex CLI proxy event", cause);
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

export function AgentList() {
  const state = useAgentState();

  if (state.status === "loading") {
    return <p>Detecting agents…</p>;
  }
  if (state.status === "error") {
    return <p>Failed to detect agents: {state.message}</p>;
  }

  return (
    <>
      <Installation name="Codex CLI" detection={state.data.codex_cli}>
        {state.data.codex_cli.status === "found" && (
          <CodexCliProxy installed={state.data.codex_cli.data.proxy_installed} />
        )}
      </Installation>
      <Installation name="Codex Desktop" detection={state.data.codex_desktop} />
    </>
  );
}
