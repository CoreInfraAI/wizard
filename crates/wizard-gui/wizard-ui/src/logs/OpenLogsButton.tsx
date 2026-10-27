import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DocumentTextIcon } from "@heroicons/react/20/solid";
import { Button } from "../components/catalyst/button";
import { SidebarItem, SidebarLabel } from "../components/catalyst/sidebar";
import { ErrorText } from "../components/ErrorText";

function useOpenLogs() {
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

  return { pending, error, open: () => void open() };
}

export function OpenLogsButton() {
  const { pending, error, open } = useOpenLogs();

  return (
    <div className="flex flex-col items-center gap-2">
      <Button plain type="button" disabled={pending} onClick={open}>
        Открыть журнал
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}

export function OpenLogsItem() {
  const { pending, error, open } = useOpenLogs();

  return (
    <>
      <SidebarItem disabled={pending} onClick={open} className="*:data-disabled:opacity-50">
        <DocumentTextIcon />
        <SidebarLabel>Журнал</SidebarLabel>
      </SidebarItem>
      {error && <div className="px-2"><ErrorText>{error}</ErrorText></div>}
    </>
  );
}
