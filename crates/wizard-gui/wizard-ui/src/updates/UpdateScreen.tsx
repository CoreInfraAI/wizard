import { AppLogo } from "../components/AppLogo";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import type { UpdateState } from "./update";

export function UpdateScreen({ state }: { state: UpdateState | undefined }) {
  const installing = state?.status === "installing";

  return (
    <div className="grid w-full max-w-sm grid-cols-1 gap-8">
      <AppLogo className="h-9 w-auto justify-self-start" />
      <div className="space-y-2">
        <Heading>{installing ? "Установка обновления…" : "Поиск обновления…"}</Heading>
        {installing && <Text>Приложение перезапустится.</Text>}
      </div>
    </div>
  );
}
