import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { Hub } from "./AgentConfiguration";
import { Button } from "../components/catalyst/button";
import { Field, Fieldset, Label, Legend } from "../components/catalyst/fieldset";
import { Select } from "../components/catalyst/select";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { refreshHubTokens } from "../settings/hub_tokens";

type VideoKey = { name: string; id: string };
type Status = {
  hub_id: string | null; key_id: string | null; key_name: string | null;
  keys_hub_id: string | null; keys: VideoKey[]; running: boolean; error: string | null;
};

export function VideoConnection({ hub, hubId, disabled }: { hub: Hub; hubId?: string | null; disabled: boolean }) {
  const [status, setStatus] = useState<Status>();
  const [draft, setDraft] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const loadedTokens = hub.status === "ready" ? hub.tokens : undefined;
  useEffect(() => {
    let active = true;
    invoke<Status>("get_video_connection").then(result => {
      if (active) setStatus(result);
    }).catch(cause => { if (active) setError(String(cause)); });
    return () => { active = false; };
  }, [hubId, loadedTokens, revision]);

  const keys = hub.signedIn && loadedTokens && hubId === status?.keys_hub_id ? status?.keys ?? [] : [];
  const selected = keys.find(key => key.id === draft) ?? keys.find(key => key.id === status?.key_id)
    ?? (keys.length === 1 ? keys[0] : undefined);
  async function apply(key: VideoKey | null) {
    setBusy(true);
    setError(undefined);
    try {
      await invoke("set_video_connection", { selection: key && { hub_id: hubId, name: key.name, id: key.id } });
      setDraft(undefined);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setRevision(value => value + 1);
      setBusy(false);
    }
  }

  return <Fieldset className="mt-10 space-y-3" disabled={busy || disabled}>
    <Legend>Видеомодели CoreInfra</Legend>
    {status?.key_name && <Text>Ключ «{status.key_name}» · {status.hub_id}</Text>}
    {hub.signedIn ? <Field>
      <Label>Имя API-ключа для видео</Label>
      <Select value={selected?.id ?? ""} onChange={event => setDraft(event.target.value)}>
        <option value="" disabled>Выберите ключ в текущем хабе</option>
        {keys.map(key => <option key={key.id} value={key.id}>{key.name}{
          keys.some(other => other.name === key.name && other.id !== key.id) ? ` · ${key.id.slice(0, 8)}` : ""
        }</option>)}
      </Select>
    </Field> : <Text>Войдите в CoreInfra Hub, чтобы выбрать ключ для видео.</Text>}
    {hub.signedIn && hub.status === "ready" && status?.keys_hub_id === hubId && !keys.length &&
      <Text>Создайте именованный API-ключ в выбранном хабе.</Text>}
    {hub.signedIn && hub.status === "error" && <ErrorText>{hub.error}</ErrorText>}
    {hub.signedIn && <Text>{status?.key_id
      ? "Оставьте Wizard запущенным и попросите ChatGPT сгенерировать видео в новом чате."
      : "Wizard установит скилл для ChatGPT / Codex. Для генерации оставьте Wizard запущенным."}</Text>}
    <div className="flex flex-wrap gap-2">
      <Button disabled={!selected || !status} onClick={() => selected && void apply(selected)}>
        {status?.key_id ? "Применить подключение" : "Подключить видеомодели"}
      </Button>
      {(status?.key_id || status?.error) && <Button outline onClick={() => void apply(null)}>Отключить</Button>}
      {hub.signedIn && <Button plain disabled={hub.status === "loading"} onClick={() => {
        setError(undefined);
        void refreshHubTokens().catch(cause => setError(String(cause)));
      }}>Обновить список ключей</Button>}
    </div>
    {status?.key_id && !status.running && !status.error && <ErrorText>Сервер видео остановлен. Примените подключение повторно.</ErrorText>}
    {(error || status?.error) && <ErrorText>{error || status?.error}</ErrorText>}
  </Fieldset>;
}
