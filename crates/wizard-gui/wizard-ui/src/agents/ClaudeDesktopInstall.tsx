import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { type TokenChoice } from "./agents_state";
import { Button } from "../components/catalyst/button";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

type Operation = "check" | "download" | "install";
type Phase =
  | { status: "unsupported" | "idle" | "checking" | "up_to_date" | "installing" }
  | { status: "available" | "ready_to_install"; version: string; path: string }
  | { status: "downloading"; received: number; total: number }
  | { status: "failed"; operation: Operation; message: string };
type Snapshot = Phase & { revision: number };

export function useClaudeDesktopInstall() {
  const [state, setState] = useState<Snapshot>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: Snapshot = await invoke<Snapshot>("get_claude_desktop_install_state", { lastRevision });
          if (stopped) return;
          setState(next);
          setError(undefined);
          if (next.status === "unsupported") return;
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setError("Не удалось получить состояние установки Claude");
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, []);

  const busy = state?.status === "checking" || state?.status === "downloading" || state?.status === "installing";
  return { state, error, busy };
}

export function ClaudeDesktopInstall({ observation, token, tokenReady, disabled, installed }: {
  observation: ReturnType<typeof useClaudeDesktopInstall>;
  token: TokenChoice | undefined;
  tokenReady: boolean;
  disabled: boolean;
  installed: boolean;
}) {
  const [error, setError] = useState<string>();
  const { state, busy } = observation;
  if (!state) return observation.error ? <ErrorText>{observation.error}</ErrorText> : null;
  if (state.status === "unsupported") return null;

  const retryInstall = state.status === "failed" && state.operation === "install";
  const blocked = disabled || busy || observation.error !== undefined;
  const networkBlocked = blocked || !tokenReady || !token;
  const progress = state.status === "downloading" && state.total > 0
    ? Math.min(100, Math.floor(state.received / state.total * 100)) : undefined;

  let operation: Operation = "check";
  let label = "Проверить обновления";
  switch (state.status) {
    case "checking":
      label = "Проверяем обновления…";
      break;
    case "available":
      operation = "download";
      label = installed ? "Скачать обновление" : "Скачать Claude";
      break;
    case "ready_to_install":
      operation = "install";
      label = installed ? "Установить обновление" : "Установить Claude";
      break;
    case "installing":
      operation = "install";
      label = "Устанавливаем…";
      break;
    case "failed":
      operation = state.operation;
      label = operation === "install" ? "Повторить установку"
        : operation === "download" ? "Повторить скачивание"
        : "Повторить проверку";
      break;
  }

  async function request(operation: Operation) {
    if (blocked || (operation !== "install" && (!token || !tokenReady))) return;
    setError(undefined);
    try {
      if (operation === "install") {
        await invoke("install_claude_desktop_release");
      } else {
        await invoke(operation === "check" ? "check_claude_desktop_release" : "download_claude_desktop_release", { token });
      }
    } catch {
      // Never log IPC arguments: they carry the selected token for network operations.
      setError("Не удалось начать операцию. Проверьте её состояние и повторите попытку.");
    }
  }

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        {state.status === "downloading" ? (
          <>
            <Button outline disabled className="overflow-hidden data-disabled:opacity-100" aria-busy="true">
              <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-0 bg-blue-500/25 transition-[width] motion-reduce:transition-none dark:bg-blue-400/25"
                style={{ width: `${progress ?? 0}%` }} />
              <span className="relative">Скачивание…</span>
            </Button>
            <progress className="sr-only" aria-label="Скачивание Claude" max={100} value={progress} />
          </>
        ) : (
          <Button outline disabled={operation === "install" ? blocked : networkBlocked}
            aria-busy={busy} onClick={() => void request(operation)}>
            {label}
          </Button>
        )}
        {!busy && operation !== "check" && (
          <Button plain disabled={networkBlocked} onClick={() => void request("check")}>Проверить заново</Button>
        )}
      </div>
      {state.status === "up_to_date" && <Text>Более новой доступной версии не найдено.</Text>}
      {state.status === "available" && (
        <Text>Доступна версия {state.version}.<br />Путь установки: <code className="break-all">{state.path}</code>.</Text>
      )}
      {state.status === "ready_to_install" && (
        <Text>Версия {state.version} скачана и готова к установке.
          <br />
          Путь установки: <code className="break-all">{state.path}</code>.</Text>
      )}
      {installed && (state.status === "ready_to_install" || retryInstall) && (
        <Text>Перед установкой полностью закройте Claude Desktop, включая значок в меню. Приложение не будет запущено автоматически.</Text>
      )}
      {state.status === "installing" && <Text>Не выходите из Wizard до завершения операции.</Text>}
      {state.status === "failed" && <ErrorText>{state.message}</ErrorText>}
      {observation.error && <ErrorText>{observation.error}</ErrorText>}
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}
