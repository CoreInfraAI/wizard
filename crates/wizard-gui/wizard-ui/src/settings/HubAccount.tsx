import { useState } from "react";
import { MenuButton } from "@headlessui/react";
import { ArrowRightEndOnRectangleIcon, ArrowRightStartOnRectangleIcon, ChevronUpIcon } from "@heroicons/react/16/solid";
import { loginHubAccount, logoutHubAccount, useHubAccount } from "./hub_account";
import { Avatar } from "../components/catalyst/avatar";
import { Dropdown, DropdownDescription, DropdownItem, DropdownLabel, DropdownMenu } from "../components/catalyst/dropdown";
import { SidebarItem } from "../components/catalyst/sidebar";
import { ErrorText } from "../components/ErrorText";

// Account state and the sign-in/out action, shared by the sidebar and agent pages.
export function useHubAccountControl() {
  const { account, error: stateError } = useHubAccount();
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string>();

  async function change() {
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

  return {
    account,
    signedIn: account?.signed_in === true,
    submitting,
    error: stateError || error || account?.error || undefined,
    change: () => void change(),
  };
}

export type HubAccountControl = ReturnType<typeof useHubAccountControl>;

// Sidebar footer: the signed-in hub with a menu, or the sign-in item.
export function HubAccountMenu({ hub }: { hub: HubAccountControl }) {
  const { account } = hub;
  const hubName = account?.profile?.personal_hub_name;

  return (
    <>
      {account?.signed_in ? (
        <Dropdown>
          <MenuButton as={SidebarItem}>
            <span className="flex min-w-0 items-center gap-3">
              <Avatar initials={(hubName ?? "C").slice(0, 1).toUpperCase()} square className="size-10 bg-zinc-900 text-white dark:bg-white dark:text-zinc-900" />
              <span className="min-w-0">
                <span className="block truncate text-sm/5 font-medium text-zinc-950 dark:text-white">{hubName ?? "CoreInfra AI Hub"}</span>
                <span className="block truncate text-xs/5 font-normal text-zinc-500 dark:text-zinc-400">
                  {hubName ? "CoreInfra AI Hub" : "Вход выполнен"}
                </span>
              </span>
            </span>
            <ChevronUpIcon />
          </MenuButton>
          <DropdownMenu className="min-w-64" anchor="top start">
            <DropdownItem disabled={hub.submitting} onClick={hub.change}>
              <ArrowRightStartOnRectangleIcon />
              <DropdownLabel>Выйти</DropdownLabel>
              <DropdownDescription>Выход из аккаунта не изменяет настройки агентов.</DropdownDescription>
            </DropdownItem>
          </DropdownMenu>
        </Dropdown>
      ) : (
        <SidebarItem disabled={!account || hub.submitting} onClick={hub.change} className="*:data-disabled:opacity-50">
          <span className="flex min-w-0 items-center gap-3">
            <Avatar src="/icon.svg" square className="size-10" />
            <span className="min-w-0">
              <span className="block truncate text-sm/5 font-medium text-zinc-950 dark:text-white">
                {!account ? (hub.error ? "Хаб недоступен" : "Загрузка…") : account.pending ? "Ожидание входа…" : "Войти"}
              </span>
              <span className="block truncate text-xs/5 font-normal text-zinc-500 dark:text-zinc-400">CoreInfra AI Hub</span>
            </span>
          </span>
          <ArrowRightEndOnRectangleIcon />
        </SidebarItem>
      )}
      {hub.error && <div className="px-2"><ErrorText>{hub.error}</ErrorText></div>}
    </>
  );
}
