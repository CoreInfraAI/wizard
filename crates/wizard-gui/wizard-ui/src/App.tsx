import { AgentList } from "./AgentList";
import { CoreinfraToken } from "./CoreinfraToken";
import { useAgentState } from "./agents_state";
import { useApplicationVersion, useStartupUpdate } from "./update";

function AgentPanel() {
  const state = useAgentState();

  if (state.status === "loading") return <p>Loading state…</p>;
  if (state.status === "error") return <p>Failed to load state: {state.message}</p>;

  return (
    <>
      <CoreinfraToken saved={state.data.coreinfra_token_set} />
      <AgentList agents={state.data.agents} />
    </>
  );
}

function App() {
  const isStartupUpdateComplete = useStartupUpdate();
  const applicationVersion = useApplicationVersion();

  if (!isStartupUpdateComplete) {
    return <main>Starting Coreinfra Wizard…</main>;
  }

  return (
    <main>
      <p>Coreinfra Wizard version: {applicationVersion ? `v${applicationVersion}` : ""}</p>
      <AgentPanel />
    </main>
  );
}

export default App;
