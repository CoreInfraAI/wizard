import type { useAgentState } from "../agents/agents_state";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Label } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { ActionBar } from "../components/ui/Layout";
import { AgentIcon, choices, isFound, type ChoiceId } from "./agents";

export function AgentSelection({
  agents,
  selected,
  onSelect,
  onContinue,
}: {
  agents: ReturnType<typeof useAgentState>;
  selected?: ChoiceId;
  onSelect: (id: ChoiceId) => void;
  onContinue: (id: ChoiceId) => void;
}) {
  const installed =
    agents.status === "ready"
      ? choices.filter((choice) => isFound(choice, agents.data))
      : [];
  const selectionReady = installed.some((choice) => choice.id === selected);
  const detectionErrors =
    agents.status === "ready" &&
    Object.values(agents.data).some(
      (value) => typeof value === "object" && value.status === "error",
    );
  return (
    <div>
      <Heading level={2} className="font-semibold!" tabIndex={-1}>
        Выберите агента
      </Heading>
      <Text className="mt-2">
        {agents.status === "loading"
          ? "Ищем установленных агентов…"
          : agents.status === "error"
            ? "Не удалось проверить приложения. Проверка повторится автоматически."
            : installed.length
              ? "Найденные на этом компьютере"
              : "Установленные агенты не найдены. Вернитесь в обзор, чтобы установить приложение."}
      </Text>
      {agents.status === "error" && (
        <div className="mt-4" role="alert">
          <ErrorText>{agents.message}</ErrorText>
        </div>
      )}
      {detectionErrors && (
        <Text className="mt-3" role="status">
          Не все приложения удалось проверить. Список обновится автоматически.
        </Text>
      )}
      {installed.length > 0 && (
        <RadioGroup
          value={selected ?? ""}
          onChange={(value) => onSelect(value as ChoiceId)}
          aria-label="Выберите агента"
          className="ui-choice-group mt-5 grid-cols-1 sm:grid-cols-2"
        >
          {installed.map((choice) => (
            <RadioField
              key={choice.id}
              className="ui-choice items-center"
              data-selected={selected === choice.id}
            >
              <Radio value={choice.id} className="mt-0!" />
              <Label className="flex cursor-pointer items-center gap-3">
                <AgentIcon choice={choice} className="size-5!" />
                {choice.name}
              </Label>
            </RadioField>
          ))}
        </RadioGroup>
      )}
      <ActionBar divider>
        <Button
          disabled={!selectionReady}
          onClick={() => {
            if (selected && selectionReady) onContinue(selected);
          }}
        >
          Настроить агента
        </Button>
      </ActionBar>
    </div>
  );
}
