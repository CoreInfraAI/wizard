import { useState } from "react";
import { type AgentId, type PreparedAgentChange, applyAgentPlan } from "./agents_state";
import { reportError } from "../logs/log";
import { ErrorText } from "../components/ErrorText";
import { Button } from "../components/catalyst/button";
import { Dialog, DialogActions, DialogBody, DialogDescription, DialogTitle } from "../components/catalyst/dialog";
import { Text } from "../components/catalyst/text";

const agentNames: Record<AgentId, string> = {
  codex: "Codex",
  claude: "Claude Code",
  pi: "Pi",
  opencode: "OpenCode",
};

export function PreparedPlanDialog({ prepared, onClose }: {
  prepared: PreparedAgentChange;
  onClose: () => void;
}) {
  const { agent, info } = prepared;
  const hasChanges = info.length > 0;
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string>();

  async function apply() {
    if (applying || !hasChanges) return;

    setApplying(true);
    setError(undefined);

    try {
      await applyAgentPlan(prepared.plan);
      onClose();
    } catch (cause: unknown) {
      const message = "Не удалось применить изменения";
      reportError(message, cause);
      setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}. Часть изменений могла быть применена.`);
    } finally {
      setApplying(false);
    }
  }

  return (
    <Dialog open onClose={() => { if (!applying) onClose(); }}>
      <DialogTitle>План изменений: {agentNames[agent]}</DialogTitle>
      {hasChanges && (
        <DialogDescription>
          Завершите текущую работу и закройте агента перед применением. Wizard выполнит следующие действия.
        </DialogDescription>
      )}
      <DialogBody>
        {hasChanges ? (
          <ol className="max-h-80 list-decimal space-y-4 overflow-y-auto pl-6 text-zinc-950 dark:text-white">
            {info.map((step, index) => (
              <li key={index} className="pl-1">
                <Text className="break-words">{step.description}</Text>
              </li>
            ))}
          </ol>
        ) : (
          <Text>Изменения не требуются.</Text>
        )}
        {error && <div className="mt-4"><ErrorText>{error}</ErrorText></div>}
      </DialogBody>
      <DialogActions>
        <Button plain disabled={applying} onClick={onClose}>
          {hasChanges ? "Отмена" : "Закрыть"}
        </Button>
        {hasChanges && <Button disabled={applying} onClick={() => void apply()}>
          {applying ? "Применение…" : "Применить"}
        </Button>}
      </DialogActions>
    </Dialog>
  );
}
