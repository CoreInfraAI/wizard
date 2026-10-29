import { useRef, useState, useSyncExternalStore, type MouseEvent } from "react";
import { MenuButton } from "@headlessui/react";
import { ComputerDesktopIcon, EllipsisHorizontalIcon, MoonIcon, SunIcon } from "@heroicons/react/20/solid";
import { loginHubAccount, logoutHubAccount, openHub, selectHub, useHubAccount } from "./hub_account";
import { getTheme, setTheme, subscribeTheme } from "./theme";
import { Dropdown, DropdownDescription, DropdownDivider, DropdownHeader, DropdownHeading, DropdownItem, DropdownLabel, DropdownMenu, DropdownSection } from "../components/catalyst/dropdown";
import { SidebarItem, SidebarLabel } from "../components/catalyst/sidebar";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { useOpenLogs } from "../logs/OpenLogsButton";

// Account state and actions are shared by the sidebar and agent pages.
export function useHubAccountControl() {
  const { account, error: stateError } = useHubAccount();
  const [submitting, setSubmitting] = useState(false);
  const [agentBusy, setAgentBusy] = useState(false);
  const [error, setError] = useState<string>();

  async function run(action: () => Promise<void>) {
    if (submitting) return;
    setSubmitting(true);
    setError(undefined);
    try {
      await action();
    } catch (cause) {
      setError(typeof cause === "string" ? cause : "Не удалось выполнить действие. Попробуйте снова.");
    } finally {
      setSubmitting(false);
    }
  }

  return {
    account,
    signedIn: account?.signed_in === true,
    submitting,
    agentBusy,
    setAgentBusy,
    error: stateError || error || account?.error || undefined,
    change: () => {
      if (account && !agentBusy) void run(account.signed_in ? logoutHubAccount : loginHubAccount);
    },
    select: (hubId: string) => {
      if (!agentBusy) void run(() => selectHub(hubId));
    },
    open: () => void run(() => openHub(getTheme())),
  };
}

export type HubAccountControl = ReturnType<typeof useHubAccountControl>;

const themes = [
  { value: "light", label: "Светлая тема", Icon: SunIcon },
  { value: "dark", label: "Темная тема", Icon: MoonIcon },
  { value: "system", label: "Системная тема", Icon: ComputerDesktopIcon },
] as const;

export function HubAccountMenu({ hub }: { hub: HubAccountControl }) {
  const { account } = hub;
  const logs = useOpenLogs();
  const typeaheadUntil = useRef(0);
  const theme = useSyncExternalStore(subscribeTheme, getTheme);
  const activeHub = account?.profile?.hubs.find((item) => item.hub_id === account.active_hub_id);
  const activeName = activeHub?.display_name?.trim() || activeHub?.hub_name || "CoreInfra AI Hub";
  const disabled = hub.submitting || hub.agentBusy;

  return (
    <>
      <div className="min-w-0 w-full">
        <Dropdown>
          <MenuButton as={SidebarItem} aria-label={`Меню аккаунта: ${activeName}`} title="Меню аккаунта и хабов"
            className="block min-w-0 w-full" onFocus={() => { typeaheadUntil.current = 0; }}>
            <SidebarLabel className="min-w-0 flex-1" title={activeName}>
              {account?.signed_in ? activeName : "CoreInfra AI Hub"}
            </SidebarLabel>
            <EllipsisHorizontalIcon />
          </MenuButton>
          <DropdownMenu anchor="top end" className="w-72! max-w-[calc(100vw-2rem)]"
            onKeyDown={(event) => {
              // Keep theme and hub selection open for keyboard activation, preserving typeahead.
              if (event.key.length === 1 && event.key !== " ") {
                typeaheadUntil.current = Date.now() + 350;
                return;
              }
              if (event.key === " " && Date.now() < typeaheadUntil.current) {
                typeaheadUntil.current = Infinity;
                return;
              }
              typeaheadUntil.current = 0;
              if (event.key !== "Enter" && event.key !== " ") return;
              const id = event.currentTarget.getAttribute("aria-activedescendant");
              const item = id ? document.getElementById(id) : null;
              if (!item?.hasAttribute("data-keep-open")) return;
              event.preventDefault();
              event.stopPropagation();
              item.click();
            }}>
            <DropdownSection aria-label="Тема" className="flex! items-center gap-1 px-1.5 py-1 sm:px-1">
              {themes.map(({ value, label, Icon }) => (
                <DropdownItem key={value} aria-label={label} title={label} data-keep-open
                  aria-current={theme === value ? "true" : undefined}
                  className={`flex! justify-center px-2! py-2! ${theme === value ? "bg-zinc-950/5 dark:bg-white/10" : ""}`}
                  onClick={(event: MouseEvent<HTMLElement>) => { event.preventDefault(); setTheme(value); }}>
                  <Icon className="m-0!" />
                </DropdownItem>
              ))}
            </DropdownSection>
            {account?.profile?.username && (
              <DropdownHeader>
                <Text className="truncate text-sm/5!">{account.profile.username}</Text>
                {account.profile.verified_email && <Text className="truncate text-xs/5!">{account.profile.verified_email}</Text>}
              </DropdownHeader>
            )}
            {account?.signed_in && (
              <DropdownSection aria-label="Хабы">
                <DropdownHeading>Хабы</DropdownHeading>
                {account.profile?.hubs.map((item) => {
                  const active = item.hub_id === account.active_hub_id;
                  const name = item.display_name?.trim() || item.hub_name;
                  return (
                    <DropdownItem key={item.hub_id} data-keep-open disabled={disabled}
                      aria-current={active ? "true" : undefined}
                      onFocus={() => { typeaheadUntil.current = 0; }}
                      onPointerMove={(event) => {
                        if (!event.currentTarget.hasAttribute("data-focus")) typeaheadUntil.current = 0;
                      }}
                      onClick={(event: MouseEvent<HTMLElement>) => {
                        event.preventDefault();
                        event.currentTarget.closest<HTMLElement>('[role="menu"]')?.focus({ preventScroll: true });
                        if (!active) hub.select(item.hub_id);
                      }}>
                      <DropdownLabel className="min-w-0 truncate" title={name}>{name}</DropdownLabel>
                      {item.is_personal && <DropdownDescription>Личный хаб</DropdownDescription>}
                      {active && <span className="col-start-5 row-start-1 text-xs text-zinc-500 group-data-focus:text-white dark:text-zinc-400">Активный</span>}
                    </DropdownItem>
                  );
                })}
                {!account.profile && <DropdownItem disabled><DropdownLabel>Загрузка хабов…</DropdownLabel></DropdownItem>}
              </DropdownSection>
            )}
            <DropdownDivider />
            <DropdownItem disabled={hub.submitting} onClick={hub.open}>
              <DropdownLabel>Перейти в хаб</DropdownLabel>
            </DropdownItem>
            <DropdownItem disabled={logs.pending} onClick={logs.open}>
              <DropdownLabel>Журнал</DropdownLabel>
            </DropdownItem>
            <DropdownDivider />
            <DropdownItem disabled={!account || disabled} onClick={hub.change}>
              <DropdownLabel>{account?.signed_in ? "Выйти" : account?.pending ? "Продолжить вход" : "Войти"}</DropdownLabel>
            </DropdownItem>
          </DropdownMenu>
        </Dropdown>
      </div>
      {hub.error && <div className="px-2"><ErrorText>{hub.error}</ErrorText></div>}
      {logs.error && <div className="px-2"><ErrorText>{logs.error}</ErrorText></div>}
    </>
  );
}
