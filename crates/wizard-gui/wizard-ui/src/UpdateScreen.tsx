import { Heading } from "./components/catalyst/heading";
import { Text } from "./components/catalyst/text";
import type { UpdateState } from "./update";

export function UpdateScreen({ state }: { state: UpdateState | undefined }) {
  if (!state || state.status === "checking") return <Text>Поиск обновления…</Text>;

  return (
    <section className="w-full max-w-xl space-y-4 rounded-lg border border-zinc-200 bg-white p-6 dark:border-zinc-800 dark:bg-zinc-900">
      {state?.status === "installing" && (
        <>
          <Heading level={2}>Установка обновления…</Heading>
          <Text>Приложение перезапустится.</Text>
        </>
      )}
    </section>
  );
}
