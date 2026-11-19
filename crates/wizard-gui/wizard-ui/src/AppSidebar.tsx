import type { ComponentType, ComponentPropsWithoutRef } from "react";
import { useClose } from "@headlessui/react";
import { Cog6ToothIcon, Squares2X2Icon } from "@heroicons/react/20/solid";
import {
  agentPaths,
  overviewPath,
  settingsPath,
  type AgentPageId,
  type AppPageId,
} from "./agents/navigation";
import { AppLogo } from "./components/AppLogo";
import {
  ClaudeIcon,
  OpenAiIcon,
  OpenCodeIcon,
  PiIcon,
} from "./components/BrandIcons";
import { Button } from "./components/catalyst/button";
import {
  Sidebar,
  SidebarBody,
  SidebarFooter,
  SidebarHeader,
  SidebarHeading,
  SidebarItem,
  SidebarLabel,
  SidebarSection,
  SidebarSpacer,
} from "./components/catalyst/sidebar";
import { type HubAccountControl, HubAccountMenu } from "./settings/HubAccount";
import type { UpdateObservation } from "./updates/update";
import { UpdateItem } from "./updates/UpdateItem";
import { SidebarDownloads } from "./SidebarDownloads";

export function AppSidebar({
  page,
  account,
  update,
  onUpdate,
}: {
  page: AppPageId;
  account: HubAccountControl;
  update: UpdateObservation;
  onUpdate: () => void;
}) {
  // Closes the sidebar dialog on narrow windows; does nothing on the desktop layout.
  const close = useClose();

  function item(
    id: AgentPageId,
    label: string,
    Icon: ComponentType<ComponentPropsWithoutRef<"svg">>,
  ) {
    return (
      <SidebarItem
        href={`#${agentPaths[id]}`}
        current={page === id}
        aria-current={page === id ? "page" : undefined}
        onClick={close}
      >
        <Icon />
        <SidebarLabel>{label}</SidebarLabel>
      </SidebarItem>
    );
  }

  return (
    <Sidebar aria-label="CoreInfra Wizard">
      <SidebarHeader className="shrink-0 mac-window:lg:pt-8">
        <Button
          plain
          href={`#${overviewPath}`}
          onClick={close}
          className="w-full justify-start! px-[calc(--spacing(2)-1px)]!"
          aria-label="CoreInfra Wizard"
        >
          <AppLogo className="h-9 max-w-full" />
        </Button>
      </SidebarHeader>
      <SidebarBody>
        <SidebarSection>
          <SidebarItem
            href={`#${overviewPath}`}
            current={page === "overview"}
            aria-current={page === "overview" ? "page" : undefined}
            onClick={close}
          >
            <Squares2X2Icon />
            <SidebarLabel>Обзор</SidebarLabel>
          </SidebarItem>
          <SidebarItem
            href={`#${settingsPath}`}
            current={page === "settings"}
            aria-current={page === "settings" ? "page" : undefined}
            onClick={close}
          >
            <Cog6ToothIcon />
            <SidebarLabel>Настройки</SidebarLabel>
          </SidebarItem>
        </SidebarSection>
        <SidebarSection>
          <SidebarHeading className="text-zinc-700 dark:text-zinc-200!">
            Агенты
          </SidebarHeading>
          {item("claude_desktop", "Claude Desktop", ClaudeIcon)}
          {item("codex", "ChatGPT / Codex", OpenAiIcon)}
          {item("claude", "Claude Code", ClaudeIcon)}
          {item("opencode", "OpenCode", OpenCodeIcon)}
          {item("pi", "Pi", PiIcon)}
        </SidebarSection>
        <SidebarSpacer />
        <SidebarSection>
          <UpdateItem state={update} onOpen={onUpdate} />
        </SidebarSection>
      </SidebarBody>
      <SidebarDownloads />
      <SidebarFooter className="min-h-20 shrink-0 justify-center gap-2">
        <HubAccountMenu hub={account} />
      </SidebarFooter>
    </Sidebar>
  );
}
