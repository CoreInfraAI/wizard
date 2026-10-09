import { useState } from "react";
import clsx from "clsx";
import { sendAgentEvent } from "../agents/agents_state";
import { useSettings } from "./settings";
import { reportError } from "../logs/log";
import { Button } from "../components/catalyst/button";
import { Badge } from "../components/catalyst/badge";
import { Input } from "../components/catalyst/input";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

export function CoreInfraToken({ className }: { className?: string }) {
  const { settings, error: settingsError } = useSettings();
  const savedToken = settings?.coreinfra_api_key ?? "";
  const ready = settings !== undefined && settingsError === undefined;
  const saved = savedToken !== "";
  const [draft, setDraft] = useState<string>();
  const token = draft ?? savedToken;
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function updateToken(value: string) {
    if (!ready || pending) return;
    const submitted = draft;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent({ SetCoreInfraToken: value });
      setDraft((current) => current === submitted ? undefined : current);
    } catch (cause: unknown) {
      const message = "Не удалось изменить токен";
      reportError(message, cause);
      setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setPending(false);
    }
  }

  return (
    <section className={clsx(className, "@container min-w-0 space-y-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900")}>
      <div className="flex flex-wrap items-center gap-2">
        <Heading level={2}>CoreInfra API токен</Heading>
        <Badge color={ready && saved ? "green" : "zinc"}>
          {settingsError ? "Недоступен" : !ready ? "Загрузка…" : saved ? "Сохранён" : "Не задан"}
        </Badge>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Input
          className="w-80! max-w-full shrink-0"
          type="text"
          autoComplete="off"
          spellCheck={false}
          value={token}
          disabled={!ready}
          onChange={(event) => {
            setDraft(event.target.value);
            setError(undefined);
          }}
        />
        <div className="flex shrink-0 gap-3">
          <Button
            outline
            type="button"
            disabled={!ready || pending || token.trim() === savedToken}
            onClick={() => void updateToken(token.trim())}
          >
            Сохранить
          </Button>
          <Button outline type="button" disabled={!ready || pending || !saved} onClick={() => void updateToken("")}>
            Удалить токен
          </Button>
        </div>
      </div>
      <Text>После замены токена нажмите «Применить» у нужных агентов.</Text>
      <Text>Удаление убирает токен только из Wizard, но не из уже настроенных агентов.</Text>
      {settingsError && <ErrorText>{settingsError}</ErrorText>}
      {error && <ErrorText>{error}</ErrorText>}
    </section>
  );
}
