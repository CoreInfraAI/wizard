import { AgentList } from "./agents/AgentList";
import { Text } from "./components/catalyst/text";
import { ErrorText } from "./components/ErrorText";
import { CoreInfraToken } from "./settings/CoreInfraToken";
import { OpenLogsButton } from "./logs/OpenLogsButton";
import { useAgentState } from "./agents/agents_state";
import { useUpdateState } from "./updates/update";
import { UpdateScreen } from "./updates/UpdateScreen";
import { UpdateTile } from "./updates/UpdateTile";

function AgentPanel() {
  const state = useAgentState();

  if (state.status === "loading") return <Text>Загрузка...</Text>;
  if (state.status === "error") return <ErrorText>Не удалось загрузить состояние агентов: {state.message}</ErrorText>;

  return <AgentList agents={state.data.agents} />;
}

function App() {
  const update = useUpdateState();
  const state = update.state;

  if (update.error === undefined && (!state || state.status === "checking" || state.status === "installing")) {
    return (
      <main className={!state || state.status === "checking"
        ? "mx-auto flex min-h-dvh max-w-6xl flex-col gap-6 px-6 pt-2 pb-6 sm:px-8 sm:pb-8"
        : "flex min-h-dvh flex-col gap-6 p-6"}
      >
        <div className={state?.status === "installing" ? "flex flex-1 items-center justify-center" : undefined}>
          <UpdateScreen state={state} />
        </div>
        <div className="mt-auto shrink-0">
          <OpenLogsButton />
        </div>
      </main>
    );
  }

  const updateTile = update.error !== undefined || state?.status === "failed" || state?.status === "available" ? (
    <UpdateTile state={update} />
  ) : null;

  return (
    <main className="mx-auto flex min-h-dvh max-w-6xl flex-col gap-6 px-6 pt-2 pb-6 sm:px-8 sm:pb-8">
      <div className="@container grid grid-cols-[repeat(auto-fit,minmax(320px,1fr))] gap-6">
        <CoreInfraToken className="@min-[664px]:col-span-2" />
        {updateTile}
        <AgentPanel />
      </div>
      <div className="mt-auto shrink-0">
        <OpenLogsButton />
      </div>
    </main>
  );
}

export default App;
