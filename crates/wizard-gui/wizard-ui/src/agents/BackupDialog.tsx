import { useEffect, useState } from "react";
import {
  type Backup,
  type BackupAgent,
  getAgentBackups,
  applyDirectChange,
} from "./agents_state";
import { Button } from "../components/catalyst/button";
import {
  Dialog,
  DialogActions,
  DialogBody,
} from "../components/catalyst/dialog";
import { Label } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { DialogHeader } from "../components/ui/DialogHeader";
import { EmptyState, StateNotice } from "../components/ui/StateNotice";

function label(backup: Backup): string {
  return `${new Date(backup.time_created).toLocaleString("ru-RU", {
    day: "2-digit",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  })} · №${backup.id}`;
}

export function BackupDialog({
  agent,
  onClose,
}: {
  agent: BackupAgent;
  onClose: () => void;
}) {
  const [pending, setPending] = useState(false);
  const [backups, setBackups] = useState<Backup[]>();
  const [selected, setSelected] = useState<number>();
  const [restored, setRestored] = useState<number>();
  const [error, setError] = useState<string>();
  const backup = backups?.find((item) => item.id === selected);

  useEffect(() => {
    let stopped = false;
    void getAgentBackups(agent)
      .then((result) => {
        if (!stopped) setBackups(result);
      })
      .catch((cause: unknown) => {
        if (!stopped)
          setError(
            `Не удалось загрузить резервные копии настроек: ${String(cause)}`,
          );
      });
    return () => {
      stopped = true;
    };
  }, [agent]);

  async function restore() {
    if (!backup || pending) return;

    setPending(true);
    setError(undefined);
    setRestored(undefined);

    try {
      await applyDirectChange({ RestoreBackup: { agent, id: backup.id } });

      setRestored(backup.id);
    } catch (cause: unknown) {
      setError(
        `Не удалось восстановить копию: ${String(cause)}. Часть файлов могла быть изменена.`,
      );
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog
      open
      onClose={() => {
        if (!pending) onClose();
      }}
    >
      <DialogHeader
        title={`Резервные копии настроек ${agent === "codex" ? "Codex" : agent === "claude_desktop" ? "Claude Desktop" : "Claude Code"}`}
        disabled={pending}
        onClose={onClose}
        description={
          <>
            Сохранённые конфигурации перед изменениями Wizard. Это не история
            каждого переключения режима.
            {agent === "claude_desktop" &&
              " Восстановится весь сохранённый профиль, включая настройки MCP, выбор профиля и режим подключения. После восстановления перезапустите Claude Desktop."}
            {agent === "codex" &&
              " Если фоновая служба Codex запущена, Wizard перезапустит её после восстановления. Это может прервать текущую работу."}
          </>
        }
      />
      <DialogBody className="space-y-3">
        {backups ? (
          backups.length === 0 ? (
            <EmptyState title="Резервных копий настроек пока нет." />
          ) : (
            <RadioGroup
              value={selected === undefined ? "" : String(selected)}
              onChange={(value) => setSelected(Number(value))}
              aria-label="Резервная копия настроек"
              className="ui-choice-group -m-1 max-h-72 overflow-y-auto p-1"
              disabled={pending}
            >
              {backups.map((item) => (
                <RadioField key={item.id} className="ui-choice">
                  <Radio value={String(item.id)} />
                  <Label>{label(item)}</Label>
                </RadioField>
              ))}
            </RadioGroup>
          )
        ) : (
          !error && <StateNotice tone="loading">Загрузка…</StateNotice>
        )}
        {restored !== undefined && (
          <StateNotice tone="success">
            Восстановлена резервная копия настроек №{restored}.
          </StateNotice>
        )}
        {error && <StateNotice tone="error">{error}</StateNotice>}
      </DialogBody>
      <DialogActions>
        <Button plain disabled={pending} onClick={onClose}>
          Закрыть
        </Button>
        <Button
          outline
          disabled={!backup || pending}
          onClick={() => void restore()}
        >
          {pending ? "Восстановление…" : "Восстановить"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
