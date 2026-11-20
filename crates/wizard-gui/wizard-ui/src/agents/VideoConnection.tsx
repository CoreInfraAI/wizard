import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Label } from "../components/catalyst/fieldset";
import { Switch, SwitchField } from "../components/catalyst/switch";
import { Code, Text } from "../components/catalyst/text";
import { ActionBar, Panel } from "../components/ui/Layout";
import { StateNotice } from "../components/ui/StateNotice";
import { ErrorText } from "../components/ErrorText";
import type { ConnectionDraft } from "./agents_state";

type Status = { enabled: boolean; running: boolean; error: string | null };

export function useVideoConnection(available: boolean) {
  const [status, setStatus] = useState<Status>();
  const [draft, setDraft] = useState<boolean>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (!available) return;
    let active = true;
    void invoke<Status>("get_video_connection").then(result => {
      if (!active) return;
      setStatus(result);
      setError(undefined);
    }, cause => {
      if (!active) return;
      setStatus(undefined);
      setError(String(cause));
    });
    return () => { active = false; };
  }, [available, revision]);

  const enabled = draft ?? status?.enabled;
  const needsApply = !!status && (enabled !== status.enabled || (enabled && !status.running) || !!status.error);
  function retry() {
    setError(undefined);
    setRevision(value => value + 1);
  }
  return { status, enabled, needsApply, error, retry, setEnabled: setDraft };
}

export function VideoConnection({ video, signedIn, disabled }: {
  video: ReturnType<typeof useVideoConnection>;
  signedIn: boolean;
  disabled: boolean;
}) {
  const { status, enabled, error } = video;
  return (
    <Panel
      title="Дополнительные модели"
      description="Будут установлены дополнительные скиллы, а Wizard будет работать как локальный MCP-сервер."
      framed={false}
    >
      <div className="agent-connection-options">
        {status && (
          <SwitchField disabled={disabled}>
            <Label>Подключить <Code className="whitespace-nowrap">coreinfra-video</Code> для видео-моделей</Label>
            <Switch checked={enabled} onChange={video.setEnabled} />
          </SwitchField>
        )}
        {!status && !error && <StateNotice tone="loading">Проверяем подключение…</StateNotice>}
        {error && <StateNotice tone="error" actions={<Button plain disabled={disabled} onClick={video.retry}>Повторить</Button>}>{error}</StateNotice>}
        {status?.running && !signedIn && <Text>Для генерации видео войдите в CoreInfra Hub.</Text>}
        {status?.error && <StateNotice tone="error">{status.error}</StateNotice>}
        {enabled && status?.enabled && !status.running && !status.error && (
          <StateNotice tone="warning">Видеосервис Wizard остановлен. Он запустится при применении настроек.</StateNotice>
        )}
      </div>
    </Panel>
  );
}

// The shared video setting is also available when Codex is not installed.
export function StandaloneVideoConnection({ signedIn, disabled, error, onPrepare }: {
  signedIn: boolean;
  disabled: boolean;
  error?: string;
  onPrepare: (draft: ConnectionDraft) => void;
}) {
  const video = useVideoConnection(true);
  return (
    <div className="mt-6 space-y-6">
      <VideoConnection video={video} signedIn={signedIn} disabled={disabled} />
      <ActionBar>
        <Button disabled={disabled || !video.needsApply} onClick={() => onPrepare({ agent: "codex", videoEnabled: video.enabled })}>
          Применить
        </Button>
      </ActionBar>
      {error && <ErrorText>{error}</ErrorText>}
    </div>
  );
}
