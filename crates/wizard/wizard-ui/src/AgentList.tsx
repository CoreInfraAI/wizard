import { type AgentDetection, useAgentState } from "./agents_state";

function Installation({ name, detection }: { name: string; detection: AgentDetection }) {
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
      <Installation name="Codex CLI" detection={state.data.codex_cli} />
      <Installation name="Codex Desktop" detection={state.data.codex_desktop} />
    </>
  );
}
