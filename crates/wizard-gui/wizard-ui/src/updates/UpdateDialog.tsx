import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { AppLogo } from "../components/AppLogo";
import { Button } from "../components/catalyst/button";
import { Dialog } from "../components/catalyst/dialog";
import { Text } from "../components/catalyst/text";
import { DialogHeader } from "../components/ui/DialogHeader";
import { ActionBar } from "../components/ui/Layout";
import { TransferCard } from "../components/ui/TransferCard";
import { ErrorText } from "../components/ErrorText";
import { megabytes } from "../agents/desktop_install_state";
import { type UpdateObservation, updateProgress, useApplicationVersion } from "./update";

export function UpdateDialog({ observation, onClose }: { observation: UpdateObservation; onClose: () => void }) {
  const currentVersion = useApplicationVersion();
  const state = observation.state;
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  const title = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (document.activeElement === document.body) title.current?.querySelector<HTMLElement>("h2")?.focus();
  }, [state?.status]);
  const downloading = state?.status === "downloading";
  const ready = state?.status === "ready_to_install";
  const installing = state?.status === "installing";
  const complete = state?.status === "up_to_date";
  const canDownload = state?.status === "available" || (state?.status === "failed" && !!state.version);
  const progress = updateProgress(state);
  const version = state && "version" in state ? state.version : undefined;
  async function run(command: "request_update" | "cancel_update" | "install_update") {
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setError(undefined);
    try { await invoke(command); }
    catch (cause) { setError(typeof cause === "string" ? cause : "Не удалось выполнить действие. Попробуйте ещё раз."); }
    finally { inFlight.current = false; setPending(false); }
  }
  return <Dialog open size="md" onClose={onClose} className="setup-dialog setup-dialog-compact">
    <div ref={title}><DialogHeader title="Обновление Wizard" closeLabel="Свернуть обновление" onClose={onClose} /></div>
    <div className="onboarding-install-grid" data-focused="true">
      <TransferCard name="Wizard" brand="wizard" icon={<AppLogo markOnly />}
        version={!downloading && !ready && !installing && currentVersion ? `Версия ${currentVersion}` : undefined}
        status={downloading ? `Скачивается${progress === undefined ? "…" : ` · ${progress}%`}`
          : ready ? `Версия ${version} — готова к установке`
          : installing ? "Устанавливается…"
          : complete ? ""
          : state?.status === "failed" ? "Не удалось обновить Wizard"
          : version ? `Доступна версия ${version}` : "Проверяем версию…"}
        detail={downloading ? `${megabytes(state.received)} / ${state.total ? megabytes(state.total) : "…"} МБ` : undefined}
        downloading={downloading} progress={progress} filled={ready || installing || complete}
        tone={complete ? "success" : state?.status === "failed" ? "error" : "default"} />
    </div>
    {(ready || installing) && <Text className="mt-4">После установки Wizard перезапустится.</Text>}
    {(error || observation.error || state?.status === "failed") && <div className="mt-4" role="alert"><ErrorText>{error ?? observation.error ?? (state?.status === "failed" ? state.message : undefined)}</ErrorText></div>}
    <ActionBar className="mt-6">
      {(downloading || ready) && <Button plain disabled={pending} onClick={() => void run("cancel_update")}>Отменить</Button>}
      {ready ? <Button disabled={pending || !!observation.error} onClick={() => void run("install_update")}>Установить</Button>
        : installing ? <Button outline disabled>Устанавливаем…</Button>
        : complete ? <Button onClick={onClose}>Готово</Button>
        : canDownload ? <Button disabled={pending || !!observation.error} onClick={() => void run("request_update")}>{state?.status === "failed" ? "Повторить" : "Скачать обновление"}</Button> : null}
    </ActionBar>
  </Dialog>;
}
