import { useApplicationVersion, useStartupUpdate } from "./update";

function App() {
  const isStartupUpdateComplete = useStartupUpdate();
  const applicationVersion = useApplicationVersion();

  if (!isStartupUpdateComplete) {
    return <main>Starting Wizard…</main>;
  }

  return (
    <main>version: {applicationVersion ? `v${applicationVersion}` : ""}</main>
  );
}

export default App;
