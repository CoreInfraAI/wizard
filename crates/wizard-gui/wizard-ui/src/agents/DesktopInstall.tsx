import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { type TokenChoice } from "./agents_state";
import { Button } from "../components/catalyst/button";
import { Checkbox, CheckboxField } from "../components/catalyst/checkbox";
import { Label } from "../components/catalyst/fieldset";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

// Keep in sync with `DesktopApp` in `crates/wizard-core/src/agents/desktop_install/mod.rs`.
export type DesktopApp = "claude" | "chatgpt";

const names: Record<DesktopApp, { name: string }> = {
  claude: { name: "Claude" },
  chatgpt: { name: "ChatGPT" },
};

type Operation = "check" | "download" | "install";
type Release = { version: string; path: string | null; notes: string[]; confirmation: string | null };
/** Bytes that a paused or interrupted download saved; downloading again continues from them. */
type Saved = { received: number; total: number };
type Phase =
  | { status: "idle" | "checking" | "up_to_date" }
  // `close`: the user agreed to close the running applications, which open again afterwards.
  | { status: "installing"; close: boolean }
  | ({ status: "available"; saved: Saved | null } & Release)
  | ({ status: "ready_to_install" } & Release)
  // `total` is 0 until known.
  | { status: "downloading"; received: number; total: number; reconnecting: boolean }
  | { status: "installed"; version: string; notices: string[] }
  | { status: "unsupported"; message: string }
  | { status: "failed"; operation: Operation; message: string; confirmation: string | null };
type Snapshot = Phase & { revision: number };

const megabytes = (bytes: number) => Math.floor(bytes / (1024 * 1024)).toLocaleString("ru-RU");
const RUNNING_POLL_MS = 2000;

/** Products that installation would close because they run now, polled while `active`. */
function useRunningApps(desktop: DesktopApp, active: boolean) {
  const [running, setRunning] = useState<string[]>([]);
  useEffect(() => {
    setRunning([]);
    if (!active) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function poll() {
      try {
        const next = await invoke<string[]>("get_desktop_running_apps", { desktop });
        if (stopped) return;
        setRunning((current) => current.join("\n") === next.join("\n") ? current : next);
      } catch {
        // Keep the last answer: installation checks again before it closes anything.
      }
      if (!stopped) timer = setTimeout(() => void poll(), RUNNING_POLL_MS);
    }
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [desktop, active]);
  return running;
}

export function useDesktopInstall(desktop: DesktopApp) {
  const [state, setState] = useState<Snapshot>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let stopped = false;
    async function observe() {
      let lastRevision: number | null = null;
      while (!stopped) {
        try {
          const next: Snapshot = await invoke<Snapshot>("get_desktop_install_state", { desktop, lastRevision });
          if (stopped) return;
          setState(next);
          setError(undefined);
          lastRevision = next.revision;
        } catch {
          if (stopped) return;
          setError(`Не удалось получить состояние установки ${names[desktop].name}`);
          lastRevision = null;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    }
    void observe();
    return () => { stopped = true; };
  }, [desktop]);

  const busy = state?.status === "checking" || state?.status === "downloading" || state?.status === "installing";
  // Hidden until the state loads, so that an unsupported application never flashes.
  const hidden = state === undefined ? error === undefined : state.status === "unsupported";
  return { desktop, state, error, busy, hidden };
}

export function DesktopInstall({ observation, token, tokenReady, disabled, installed }: {
  observation: ReturnType<typeof useDesktopInstall>;
  token: TokenChoice | undefined;
  tokenReady: boolean;
  disabled: boolean;
  installed: boolean;
}) {
  const [error, setError] = useState<string>();
  const [confirmed, setConfirmed] = useState(false);
  const { desktop, state, busy } = observation;
  const { name } = names[desktop];
  const confirmation = state?.status === "ready_to_install" || state?.status === "failed" ? state.confirmation : null;
  // A confirmation applies only to the release it was given for.
  useEffect(() => {
    if (confirmation === null) setConfirmed(false);
  }, [confirmation]);
  const retryInstall = state?.status === "failed" && state.operation === "install";
  const installable = state?.status === "ready_to_install" || retryInstall;
  const running = useRunningApps(desktop, installable && !busy);
  if (!state) return observation.error ? <ErrorText>{observation.error}</ErrorText> : null;
  if (state.status === "unsupported") return <Text>{state.message}</Text>;

  // Installation closes these and opens them again afterwards.
  const closing = installable && running.length > 0 ? running.join(", ") : undefined;
  const blocked = disabled || busy || observation.error !== undefined;
  const networkBlocked = blocked || !tokenReady || !token;
  const unconfirmed = confirmation !== null && !confirmed;
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
      label = state.saved
        ? `Продолжить скачивание (${megabytes(state.saved.received)} из ${megabytes(state.saved.total)} МБ)`
        : installed ? "Скачать обновление" : `Скачать ${name}`;
      break;
    case "ready_to_install":
      operation = "install";
      label = closing ? `Закрыть ${closing} и установить`
        : installed ? "Установить обновление" : `Установить ${name}`;
      break;
    case "installing":
      operation = "install";
      label = state.close ? "Закрываем и устанавливаем…" : "Устанавливаем…";
      break;
    case "failed":
      operation = state.operation;
      label = operation === "install" ? (closing ? `Закрыть ${closing} и повторить установку` : "Повторить установку")
        : operation === "download" ? "Повторить скачивание"
        : "Повторить проверку";
      break;
  }

  async function request(operation: Operation) {
    if (blocked || (operation !== "install" && (!token || !tokenReady))) return;
    if (operation === "install" && unconfirmed) return;
    setError(undefined);
    try {
      if (operation === "install") {
        await invoke("install_desktop_release", {
          desktop, confirmed: confirmation !== null && confirmed, close: closing !== undefined,
        });
      } else {
        await invoke(operation === "check" ? "check_desktop_release" : "download_desktop_release", { desktop, token });
      }
    } catch {
      // Never log IPC arguments: they carry the selected token for network operations.
      setError("Не удалось начать операцию. Проверьте её состояние и повторите попытку.");
    }
  }

  async function pause() {
    setError(undefined);
    try {
      await invoke("pause_desktop_download", { desktop });
    } catch {
      setError("Не удалось приостановить скачивание. Проверьте его состояние и повторите попытку.");
    }
  }

  const release = state.status === "available" || state.status === "ready_to_install" ? state : undefined;
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
            <progress className="sr-only" aria-label={`Скачивание ${name}`} max={100} value={progress} />
            <Button plain disabled={disabled || observation.error !== undefined} onClick={() => void pause()}>
              Приостановить
            </Button>
          </>
        ) : (
          <Button outline disabled={operation === "install" ? blocked || unconfirmed : networkBlocked}
            aria-busy={busy} onClick={() => void request(operation)}>
            {label}
          </Button>
        )}
        {!busy && operation !== "check" && (
          <Button plain disabled={networkBlocked} onClick={() => void request("check")}>Проверить заново</Button>
        )}
      </div>
      {state.status === "downloading" && (state.reconnecting
        ? <Text>Соединение прервано. Переподключаемся, скачанное сохранено…</Text>
        : state.total > 0 && <Text>Скачано {megabytes(state.received)} из {megabytes(state.total)} МБ.</Text>)}
      {state.status === "up_to_date" && <Text>Более новой доступной версии не найдено.</Text>}
      {state.status === "available" && (
        <Text>Доступна версия {state.version}.
          {state.path && <><br />Путь установки: <code className="break-all">{state.path}</code>.</>}</Text>
      )}
      {state.status === "ready_to_install" && (
        <Text>Версия {state.version} скачана и готова к установке.
          {state.path && <><br />Путь установки: <code className="break-all">{state.path}</code>.</>}</Text>
      )}
      {release?.notes.map((note) => <Text key={note}>{note}</Text>)}
      {state.status === "available" && state.confirmation !== null && (
        <Text>{state.confirmation} — Wizard попросит подтвердить это перед установкой.</Text>
      )}
      {state.status === "ready_to_install" && state.path === null && (
        <Text>Система запросит права администратора для установки {name}.</Text>
      )}
      {confirmation !== null && (state.status === "ready_to_install" || retryInstall) && (
        <CheckboxField disabled={blocked}>
          <Checkbox checked={confirmed} onChange={setConfirmed} />
          <Label>{confirmation}</Label>
        </CheckboxField>
      )}
      {closing && (
        <Text>Wizard закроет {closing} и откроет {running.length > 1 ? "их" : "его"} снова после установки. Текущие ответы и сессии будут прерваны.</Text>
      )}
      {state.status === "installing" && <Text>Не выходите из Wizard до завершения операции.</Text>}
      {state.status === "installed" && (
        <>
          <Text>Версия {state.version} установлена.</Text>
          {state.notices.map((notice) => <Text key={notice}>{notice}</Text>)}
        </>
      )}
      {state.status === "failed" && <ErrorText>{state.message}</ErrorText>}
      {observation.error && <ErrorText>{observation.error}</ErrorText>}
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}
