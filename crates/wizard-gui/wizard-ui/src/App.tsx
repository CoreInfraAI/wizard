import { AgentList } from "./AgentList";
import { Text } from "./components/catalyst/text";
import { ErrorText } from "./components/ErrorText";
import { CoreinfraToken } from "./CoreinfraToken";
import { useAgentState } from "./agents_state";
import { useStartupUpdate } from "./update";

function AgentPanel() {
  const state = useAgentState();

  if (state.status === "loading") return <Text>Loading state…</Text>;
  if (state.status === "error") return <ErrorText>Не удалось загрузить состояние агентов: {state.message}</ErrorText>;

  return (
    <>
      <CoreinfraToken saved={state.data.coreinfra_token_set} />
      <AgentList agents={state.data.agents} />
    </>
  );
}

function App() {
  const isStartupUpdateComplete = useStartupUpdate();

  if (!isStartupUpdateComplete) {
    return (
      <main className="mx-auto max-w-6xl px-6 pt-2 pb-6 sm:px-8 sm:pb-8">
        <Text>Starting Coreinfra Wizard…</Text>
      </main>
    );
  }

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 pt-2 pb-6 sm:px-8 sm:pb-8">
      <AgentPanel />
    </main>
  );
}

export default App;
