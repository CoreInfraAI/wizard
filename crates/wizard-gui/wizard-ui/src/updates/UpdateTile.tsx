import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { reportError } from "../logs/log";
import { type UpdateObservation, requestUpdate, retryUpdateCheck } from "./update";

export function UpdateTile({ state }: {
  state: UpdateObservation;
}) {
  const current = state.state;
  const error = state.error ?? (current?.status === "failed" ? current.message : undefined);

  if (error === undefined && current?.status !== "available") return null;

  function request() {
    const action = current?.status === "failed" ? retryUpdateCheck : requestUpdate;
    void action().catch((cause: unknown) => {
      reportError("Не удалось запустить обновление", cause);
    });
  }

  return (
    <section className="min-w-0 space-y-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900">
      <Heading level={2}>Обновление Wizard</Heading>
      {error !== undefined
        ? <ErrorText>{error}</ErrorText>
        : current?.status === "available" && <Text>Доступна версия v{current.version}</Text>}
      {state.error === undefined && (
        <Button outline type="button" onClick={request}>
          {error !== undefined ? "Повторить" : "Обновить"}
        </Button>
      )}
    </section>
  );
}
