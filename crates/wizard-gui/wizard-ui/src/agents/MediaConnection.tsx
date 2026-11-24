import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Label } from "../components/catalyst/fieldset";
import { Switch, SwitchField } from "../components/catalyst/switch";
import { Code } from "../components/catalyst/text";
import { ActionBar, Panel } from "../components/ui/Layout";
import { StateNotice } from "../components/ui/StateNotice";
import { ErrorText } from "../components/ErrorText";
import type { AgentId, ConnectionDraft } from "./agents_state";

// `suggested`: no grant yet and never turned off by the user, so the form offers media tools by default.
type Status = { enabled: boolean; suggested: boolean; running: boolean; error: string | null };

export function useMediaConnection(agent: AgentId, available: boolean) {
  const [status, setStatus] = useState<Status>();
  const [draft, setDraft] = useState<boolean>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (!available) return;
    let active = true;
    void invoke<Status>("get_media_connection", { agent }).then(result => {
      if (!active) return;
      setStatus(result);
      setError(undefined);
    }, cause => {
      if (!active) return;
      setStatus(undefined);
      setError(String(cause));
    });
    return () => { active = false; };
  }, [agent, available, revision]);

  const enabled = draft ?? (status && (status.enabled || status.suggested));
  // The first Apply also saves an explicit decline of the default suggestion.
  const needsApply = !!status && (status.suggested || enabled !== status.enabled || (enabled && !status.running) || !!status.error);
  function retry() {
    setError(undefined);
    setRevision(value => value + 1);
  }
  return { status, enabled, needsApply, error, retry, setEnabled: setDraft };
}

export function MediaConnection({ media, disabled }: {
  media: ReturnType<typeof useMediaConnection>;
  disabled: boolean;
}) {
  const { status, enabled, error } = media;
  return (
    <Panel
      title="Дополнительные модели"
      description="Будут установлены дополнительные скиллы, а Wizard будет работать как локальный MCP-сервер."
      framed={false}
    >
      <div className="agent-connection-options">
        {status && (
          <SwitchField disabled={disabled}>
            <Label>Подключить <Code className="whitespace-nowrap">coreinfra-media</Code> для видео, озвучки и распознавания речи</Label>
            <Switch checked={enabled} onChange={media.setEnabled} />
          </SwitchField>
        )}
        {!status && !error && <StateNotice tone="loading">Проверяем подключение…</StateNotice>}
        {error && <StateNotice tone="error" actions={<Button plain disabled={disabled} onClick={media.retry}>Повторить</Button>}>{error}</StateNotice>}
        {status?.error && <StateNotice tone="error">{status.error}</StateNotice>}
        {enabled && status?.enabled && !status.running && !status.error && (
          <StateNotice tone="warning">Медиасервис Wizard остановлен. Он запустится при применении настроек.</StateNotice>
        )}
      </div>
    </Panel>
  );
}

export function StandaloneMediaConnection({ agent, disabled, error, onPrepare }: {
  agent: AgentId;
  disabled: boolean;
  error?: string;
  onPrepare: (draft: ConnectionDraft) => void;
}) {
  const media = useMediaConnection(agent, true);
  return (
    <div className="mt-6 space-y-6">
      <MediaConnection media={media} disabled={disabled} />
      <ActionBar>
        <Button disabled={disabled || !media.needsApply} onClick={() => onPrepare({ agent, mediaEnabled: media.enabled })}>
          Применить
        </Button>
      </ActionBar>
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}
