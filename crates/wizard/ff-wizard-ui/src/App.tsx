import { getVersion } from "@tauri-apps/api/app";
import { useEffect, useState } from "react";

function App() {
  const [version, setVersion] = useState<string>();

  useEffect(() => {
    let active = true;

    getVersion()
      .then((appVersion) => {
        if (active) {
          setVersion(appVersion);
        }
      })
      .catch((error: unknown) => {
        console.error("failed to get application version", error);
      });

    return () => {
      active = false;
    };
  }, []);

  return <main aria-live="polite">version: {version ? `v${version}` : ""}</main>;
}

export default App;
