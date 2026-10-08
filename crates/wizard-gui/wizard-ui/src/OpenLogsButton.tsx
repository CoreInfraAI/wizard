import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "./components/catalyst/button";
import { ErrorText } from "./components/ErrorText";

export function OpenLogsButton() {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function open() {
    setPending(true);
    setError(undefined);
    try {
      await invoke("open_logs");
    } catch (cause: unknown) {
      setError(`Не удалось открыть журнал: ${String(cause)}`);
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="flex flex-col items-end gap-2">
      <Button plain type="button" disabled={pending} onClick={() => void open()}>
        Открыть журнал
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}
