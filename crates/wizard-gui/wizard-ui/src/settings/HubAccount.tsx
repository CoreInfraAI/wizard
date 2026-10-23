import { useState } from "react";
import clsx from "clsx";
import { loginHubAccount, logoutHubAccount, useHubAccount } from "./hub_account";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Strong, Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

export function HubAccount({ className }: { className?: string }) {
  const { account, error: stateError } = useHubAccount();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string>();

  async function changeAccount() {
    if (!account || submitting) return;
    setSubmitting(true);
    setError(undefined);
    try {
      // While pending, login reopens the Hub page and extends the wait.
      await (account.signed_in ? logoutHubAccount() : loginHubAccount());
    } catch (cause) {
      setError(typeof cause === "string" ? cause : "Не удалось изменить состояние входа. Попробуйте снова.");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <section className={clsx(className, "min-w-0 space-y-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900")}>
      <div className="flex flex-wrap items-center gap-2">
        <Heading level={2}>CoreInfra AI Hub</Heading>
        <Badge color={account?.signed_in ? "green" : "zinc"}>
          {stateError ? "Недоступен" : !account ? "Загрузка…" : account.pending ? "Ожидание входа…" : account.signed_in ? "Вход выполнен" : "Вход не выполнен"}
        </Badge>
      </div>
      {account?.signed_in ? (
        <div className="flex items-center gap-3">
          {account.profile && (
            <Text className="min-w-0 break-words">
              <Strong>Хаб: {account.profile.personal_hub_name}</Strong>
            </Text>
          )}
          <Button plain type="button" className="shrink-0" disabled={submitting} onClick={() => void changeAccount()}>
            Выйти
          </Button>
        </div>
      ) : (
        <Button outline type="button" disabled={!account || submitting} onClick={() => void changeAccount()}>
          Войти через CoreInfra Hub
        </Button>
      )}
      <Text>{account?.signed_in ? "Выход из аккаунта не изменяет настройки агентов." : "Вход в аккаунт не изменяет настройки агентов."}</Text>
      {(stateError || error || account?.error) && <ErrorText>{stateError || error || account?.error}</ErrorText>}
    </section>
  );
}
