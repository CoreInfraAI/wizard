import type { ReactNode } from "react";
import { AgentList } from "./AgentList";
import { Text } from "./components/catalyst/text";
import { ErrorText } from "./components/ErrorText";
import { CoreinfraToken } from "./CoreinfraToken";
import { useAgentState } from "./agents_state";
import { requestUpdate, useUpdateState } from "./update";
import { UpdateScreen } from "./UpdateScreen";
import { UpdateTile } from "./UpdateTile";

function AgentPanel({ updateTile }: { updateTile: ReactNode }) {
  const state = useAgentState();

  if (state.status === "loading") return <Text>Загрузка...</Text>;
  if (state.status === "error") return <ErrorText>Не удалось загрузить состояние агентов: {state.message}</ErrorText>;

  return (
    <>
      <div className={updateTile
        ? "grid grid-cols-[minmax(0,2fr)_minmax(0,1fr)] gap-6"
        : "grid grid-cols-1"}
      >
        <CoreinfraToken saved={state.data.coreinfra_token_set} />
        {updateTile}
      </div>
      <AgentList agents={state.data.agents} />
    </>
  );
}

function App() {
  const update = useUpdateState();
  const state = update.state;

  if (update.error === undefined && (!state || state.status === "checking" || state.status === "installing")) {
    return (
      <main className={!state || state.status === "checking"
        ? "mx-auto max-w-6xl space-y-6 px-6 pt-2 pb-6 sm:px-8 sm:pb-8"
        : "flex min-h-dvh items-center justify-center p-6"}
      >
        <UpdateScreen state={state} />
      </main>
    );
  }

  const updateTile = update.error !== undefined || state?.status === "failed" || state?.status === "available" ? (
    <UpdateTile state={update} update={requestUpdate} />
  ) : null;

  return (
    <main className="mx-auto max-w-6xl space-y-6 px-6 pt-2 pb-6 sm:px-8 sm:pb-8">
      <AgentPanel updateTile={updateTile} />
    </main>
  );
}

export default App;
