import { useEffect, useState } from "react";
import { type Backup, type BackupAgent, getAgentBackups, sendAgentEvent } from "./agents_state";
import { Button } from "../components/catalyst/button";
import { Dialog, DialogActions, DialogBody, DialogDescription, DialogTitle } from "../components/catalyst/dialog";
import { Label } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

function label(backup: Backup): string {
  return `${new Date(backup.time_created).toLocaleString("ru-RU", {
    day: "2-digit",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  })} · №${backup.id}`;
}

export function BackupDialog({ agent, onClose }: {
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
    void getAgentBackups(agent).then((result) => {
      if (!stopped) setBackups(result);
    }).catch((cause: unknown) => {
      if (!stopped) setError(`Не удалось загрузить резервные копии: ${String(cause)}`);
    });
    return () => { stopped = true; };
  }, [agent]);

  async function restore() {
    if (!backup || pending) return;
    setPending(true);
    setError(undefined);
    setRestored(undefined);
    try {
      await sendAgentEvent({ RestoreBackup: { agent, id: backup.id } });
      setRestored(backup.id);
    } catch (cause: unknown) {
      setError(`Не удалось восстановить копию: ${String(cause)}. Часть файлов могла быть изменена.`);
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open onClose={() => { if (!pending) onClose(); }}>
      <DialogTitle>Резервные копии {agent === "codex" ? "Codex" : "Claude Code"}</DialogTitle>
      <DialogDescription>
        Сохранённые конфигурации перед изменениями Wizard. Это не история каждого переключения режима.
      </DialogDescription>
      <DialogBody className="space-y-3">
        {backups ? (
          backups.length === 0 ? (
            <Text>Резервных копий пока нет.</Text>
          ) : (
            <RadioGroup value={selected === undefined ? "" : String(selected)} onChange={(value) => setSelected(Number(value))} aria-label="Резервная копия" className="max-h-72 overflow-y-auto">
              {backups.map((item) => (
                <RadioField key={item.id}>
                  <Radio value={String(item.id)} />
                  <Label>{label(item)}</Label>
                </RadioField>
              ))}
            </RadioGroup>
          )
        ) : !error && <Text role="status">Загрузка…</Text>}
        {restored !== undefined && <Text role="status">Восстановлена резервная копия №{restored}.</Text>}
        {error && <ErrorText>{error}</ErrorText>}
      </DialogBody>
      <DialogActions>
        <Button plain disabled={pending} onClick={onClose}>
          Закрыть
        </Button>
        <Button outline disabled={!backup || pending} onClick={() => void restore()}>
          {pending ? "Восстановление…" : "Восстановить"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
