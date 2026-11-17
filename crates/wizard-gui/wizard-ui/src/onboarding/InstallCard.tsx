import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { Button } from "../components/catalyst/button";
import { Checkbox, CheckboxField } from "../components/catalyst/checkbox";
import { Label } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { useDesktopInstall } from "../agents/DesktopInstall";
import { AgentIcon, type AgentChoice } from "./agents";

type Observation = ReturnType<typeof useDesktopInstall>;

/** Downloading ends at a separate confirmation; installation is always explicit. */
export function InstallCard({ choice, observation, signedIn, found, version, hidden, onStart, onDone }: {
  choice: AgentChoice;
  observation: Observation;
  signedIn: boolean;
  found: boolean;
  version: string | null;
  hidden: boolean;
  onStart: () => void;
  onDone: () => void;
}) {
  const { desktop, state } = observation;
  const [started, setStarted] = useState(false);
  const [paused, setPaused] = useState(false);
  const [error, setError] = useState<string>();
  const [gate, setGate] = useState<{ running: string[]; confirmation: string | null }>();
  const [confirmed, setConfirmed] = useState(false);
  const [requesting, setRequesting] = useState(false);
  const attempted = useRef<number | undefined>(undefined);
  const restored = useRef(false);
  const inFlight = useRef(false);
  const lastProgress = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  // Reopening restores the operation without resuming an explicit pause.
  useEffect(() => {
    if (!state || restored.current) return;
    restored.current = true;
    if (observation.busy || state.status === "failed") setStarted(true);
    if (state.status === "available" && state.saved) { setStarted(true); setPaused(true); }
  }, [state, observation.busy]);

  async function request(command: string, args: Record<string, unknown> = {}) {
    if (inFlight.current) return;
    inFlight.current = true;
    setRequesting(true);
    setError(undefined);
    try {
      await invoke(command, { desktop, ...(command === "install_desktop_release" ? { openAfterInstall: false } : {}), ...args });
    } catch (cause) {
      if (mounted.current) setError(typeof cause === "string" ? cause : "Не удалось начать операцию. Попробуйте ещё раз.");
    } finally {
      inFlight.current = false;
      if (mounted.current) setRequesting(false);
    }
  }

  useEffect(() => {
    if (!started || !state || paused || requesting || inFlight.current || error || observation.error) return;
    if ((state.status === "idle" || state.status === "available") && !signedIn) return;
    if (attempted.current === state.revision) return;
    attempted.current = state.revision;
    if (state.status === "idle") void request("check_desktop_release");
    if (state.status === "available") void request("download_desktop_release");
    if (state.status === "ready_to_install") {
      // Prepare the confirmation, but never install as a side effect of downloading.
      inFlight.current = true;
      setRequesting(true);
      void invoke<string[]>("get_desktop_running_apps", { desktop }).then((running) => {
        inFlight.current = false;
        if (!mounted.current) return;
        setConfirmed(false);
        setGate({ running, confirmation: state.confirmation });
        setRequesting(false);
      }).catch(() => {
        inFlight.current = false;
        if (mounted.current) {
          setError("Не удалось проверить запущенные приложения. Повторите попытку.");
          setRequesting(false);
        }
      });
    }
  });

  const downloading = state?.status === "downloading";
  const installed = state?.status === "installed";
  const downloaded = state?.status === "ready_to_install";
  const upToDate = state?.status === "up_to_date";
  const completed = started && (installed || upToDate);
  const installedVersion = installed ? state.version : version;
  const installing = state?.status === "installing";
  const failed = state?.status === "failed";
  const failedMessage = error ?? observation.error ?? (failed ? state.message : undefined);
  const progress = downloading && state.total > 0 ? Math.min(100, Math.floor(state.received / state.total * 100)) : undefined;
  const saved = state?.status === "available" ? state.saved : null;
  const displayProgress = progress ?? (saved && saved.total > 0 ? Math.min(100, Math.floor(saved.received / saved.total * 100)) : undefined);
  useEffect(() => { if (displayProgress !== undefined) lastProgress.current = displayProgress; }, [displayProgress]);
  const active = started || observation.busy;
  const fill = !active ? 0 : completed || installing || state?.status === "ready_to_install" ? 100 : displayProgress ?? lastProgress.current;
  const unsupported = state?.status === "unsupported";
  const networkBlocked = !signedIn || Boolean(observation.error);
  const description = desktop === "claude" ? "Приложение Anthropic с Cowork и Claude\u00a0Code." : "Приложение OpenAI с ChatGPT\u00a0Work и Codex.";
  const title = failedMessage ? "Установка прервана" : !active ? downloaded ? `Скачана версия ${state.version}` : found || installed ? "" : description : upToDate ? "Версия актуальна" : installed ? "Установлен"
    : paused ? `На паузе${displayProgress === undefined ? "" : ` · ${displayProgress}%`}` : installing ? "Устанавливается…" : gate ? "Подтвердите установку" : downloading ? `Скачивается${progress === undefined ? "…" : ` ${progress}%`}` : state?.status === "ready_to_install" ? "Подготавливаем установку…" : "Проверяем обновления…";
  const bytes = downloading ? state : paused ? saved : null;
  const detail = downloading && state.reconnecting ? "Восстанавливаем соединение…" : bytes && bytes.total > 0 ? `${Math.floor(bytes.received / 1048576)} из ${Math.floor(bytes.total / 1048576)} МБ` : undefined;

  async function pause() {
    setPaused(true);
    await request("pause_desktop_download");
  }

  function start() {
    if (inFlight.current || (downloaded ? observation.error : networkBlocked)) return;
    onStart();
    setStarted(true);
    setPaused(false);
    setGate(undefined);
    attempted.current = downloaded ? undefined : state?.revision;
    lastProgress.current = 0;
    if (downloaded) return;
    // An installed app follows the same entry point; the backend compares versions.
    void request("check_desktop_release");
  }

  if (active && downloaded) return <div className="onboarding-install-confirmation" hidden={hidden}>
    <div className="flex items-center gap-3">
      <AgentIcon choice={choice} />
      <Heading level={2} className="font-semibold!" tabIndex={-1}>{choice.name} версии {state.version} скачан.</Heading>
    </div>
    {state.notes.length > 0 && <div className="mt-4 space-y-2">{state.notes.map((note) => <Text key={note}>{note}</Text>)}</div>}
    {!state.path && <Text className="mt-4">Система запросит права администратора для установки {choice.name}.</Text>}
    {gate && gate.running.length > 0 && <Text className="mt-4">Wizard закроет {gate.running.join(", ")} и откроет снова. Сохраните текущую работу перед установкой.</Text>}
    {gate?.confirmation && <CheckboxField className="mt-4"><Checkbox checked={confirmed} onChange={setConfirmed} /><Label>{gate.confirmation}</Label></CheckboxField>}
    {failedMessage && <div className="mt-4 space-y-2" role="alert"><ErrorText>{failedMessage}</ErrorText>{!gate && <Button plain disabled={requesting} onClick={retry}>Повторить</Button>}</div>}
    <div className="onboarding-footer flex justify-end gap-3">
      <Button plain disabled={requesting} onClick={onDone}>Отмена</Button>
      <Button disabled={requesting || !gate || (Boolean(gate.confirmation) && !confirmed)} onClick={() => void request("install_desktop_release", { confirmed, close: Boolean(gate?.running.length) })}>Установить</Button>
    </div>
  </div>;

  function retry() {
    onStart();
    setError(undefined);
    attempted.current = undefined;
    setPaused(false);
    setStarted(true);
    if (failed) {
      // A new release check also recovers an archive rejected by the installer.
      setGate(undefined);
      void request("check_desktop_release");
    }
  }

  return <article className="onboarding-install-card" data-app={desktop} data-active={active} aria-label={choice.name} hidden={hidden}
    style={{ "--install-progress": `${fill}%` } as CSSProperties}>
    <div aria-hidden="true" className="onboarding-download-fill" />
    <div className="onboarding-install-main">
      <div className="onboarding-install-copy">
        <div className="onboarding-install-header">
          <div className="onboarding-install-details">
            <Heading level={3} className="font-semibold!">{choice.name}</Heading>
            {(found || installed) && <Text className="mt-1 tabular-nums">{installedVersion ? `Версия ${installedVersion}` : "Версия не определена"}</Text>}
            <Text className={found || completed ? "" : "mt-1"} role="status" aria-live="polite">{title}</Text>
            {detail && <Text className="tabular-nums">{detail}</Text>}
          </div>
          <div className="onboarding-install-logo" aria-hidden="true"><div className="onboarding-install-mark"><AgentIcon choice={choice} /></div></div>
          {/* Extend across the card so logo color follows the download fill exactly. */}
          <div className="onboarding-install-color" aria-hidden="true"><div className="onboarding-install-logo"><div className="onboarding-install-mark"><AgentIcon choice={choice} /></div></div></div>
        </div>
        <div className="onboarding-install-actions">
          {failedMessage ? <Button outline disabled={requesting || networkBlocked} aria-label="Попробовать снова" onClick={retry}>Повторить</Button>
              : paused ? <Button outline disabled={requesting || downloading || networkBlocked} aria-label="Продолжить скачивание" onClick={retry}>Продолжить</Button>
                : downloading ? <Button outline disabled={requesting} onClick={() => void pause()}>Приостановить</Button>
                  : completed ? <Button outline onClick={onDone}>Готово</Button>
                    : !active && <Button outline disabled={requesting || !state || unsupported || (downloaded ? Boolean(observation.error) : networkBlocked)} onClick={start}>{unsupported ? "Недоступно" : downloaded ? "Установить" : found || installed ? "Проверить обновления" : "Скачать"}</Button>}
        </div>
      </div>
    </div>
    {(downloading || paused) && <progress className="sr-only" aria-label={`Скачивание ${choice.name}`} max={100} value={displayProgress} />}
    {failedMessage && <div className="onboarding-install-note" role="alert"><ErrorText>{failedMessage}</ErrorText></div>}
    {!found && state?.status === "unsupported" && <Text className="onboarding-install-note">{state.message}</Text>}
    {installed && state.notices.length > 0 && <div className="onboarding-install-note space-y-2">{state.notices.map((notice) => <Text key={notice}>{notice}</Text>)}</div>}
  </article>;
}
