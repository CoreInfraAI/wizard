import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Fieldset } from "../components/catalyst/fieldset";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

type Status = { enabled: boolean; running: boolean; error: string | null };

export function VideoConnection({ signedIn, disabled }: { signedIn: boolean; disabled: boolean }) {
  const [status, setStatus] = useState<Status>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true;
    invoke<Status>("get_video_connection").then(result => {
      if (active) setStatus(result);
    }).catch(cause => { if (active) setError(String(cause)); });
    return () => { active = false; };
  }, [revision]);

  async function apply(enabled: boolean) {
    setBusy(true);
    setError(undefined);
    try {
      await invoke("set_video_connection", { enabled });
    } catch (cause) {
      setError(String(cause));
    } finally {
      setRevision(value => value + 1);
      setBusy(false);
    }
  }

  if (!status && !error) return null;

  return <Fieldset aria-label="Видеомодели" className="mt-10 space-y-3" disabled={busy || disabled}>
    {status?.running && !signedIn && <Text>Для генерации войдите в CoreInfra Hub в Wizard.</Text>}
    {status && !status.enabled && <Text>Видеомодели отключены.</Text>}
    {(error || status?.error) && <ErrorText>{error || status?.error}</ErrorText>}
    {status && <div className="flex flex-wrap gap-2">
      {!status.running && <Button onClick={() => void apply(true)}>
        {status.enabled ? "Переподключить видеомодели" : "Включить видеомодели"}
      </Button>}
      {status.enabled && <Button outline onClick={() => void apply(false)}>Отключить видеомодели</Button>}
    </div>}
  </Fieldset>;
}
