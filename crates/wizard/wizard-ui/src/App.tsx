import { AgentList } from "./AgentList";
import { useApplicationVersion, useStartupUpdate } from "./update";

function App() {
  const isStartupUpdateComplete = useStartupUpdate();
  const applicationVersion = useApplicationVersion();

  if (!isStartupUpdateComplete) {
    return <main>Starting Wizard…</main>;
  }

  return (
    <main>
      <p>Wizard version: {applicationVersion ? `v${applicationVersion}` : ""}</p>
      <AgentList />
    </main>
  );
}

export default App;
