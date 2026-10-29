import type { ComponentType, ComponentPropsWithoutRef } from "react";
import { useClose } from "@headlessui/react";
import { agentPaths, settingsPath, type AgentPageId, type AppPageId } from "./agents/navigation";
import { AppLogo } from "./components/AppLogo";
import { ClaudeIcon, OpenAiIcon, OpenCodeIcon, PiIcon } from "./components/BrandIcons";
import { Button } from "./components/catalyst/button";
import { Divider } from "./components/catalyst/divider";
import { Sidebar, SidebarBody, SidebarFooter, SidebarHeader, SidebarHeading, SidebarItem, SidebarLabel, SidebarSection, SidebarSpacer } from "./components/catalyst/sidebar";
import { type HubAccountControl, HubAccountMenu } from "./settings/HubAccount";
import type { UpdateObservation } from "./updates/update";
import { UpdateItem } from "./updates/UpdateItem";

export function AppSidebar({ page, account, update }: {
  page: AppPageId;
  account: HubAccountControl;
  update: UpdateObservation;
}) {
  // Closes the sidebar dialog on narrow windows; does nothing on the desktop layout.
  const close = useClose();

  function item(id: AgentPageId, label: string, Icon: ComponentType<ComponentPropsWithoutRef<"svg">>) {
    return (
      <SidebarItem href={`#${agentPaths[id]}`} current={page === id} aria-current={page === id ? "page" : undefined}>
        <Icon />
        <SidebarLabel>{label}</SidebarLabel>
      </SidebarItem>
    );
  }

  return (
    <Sidebar aria-label="CoreInfra Wizard">
      <SidebarHeader className="shrink-0 mac-window:lg:pt-8">
        <Button plain href={`#${agentPaths.claude_desktop}`} onClick={close}
          className="w-full justify-start! px-[calc(--spacing(2)-1px)]!" aria-label="CoreInfra Wizard">
          <AppLogo className="h-9 max-w-full" />
        </Button>
      </SidebarHeader>
      <SidebarBody>
        <SidebarSection>
          <SidebarHeading className="text-zinc-700 dark:text-zinc-200!">Агенты</SidebarHeading>
          {item("claude_desktop", "Claude Desktop", ClaudeIcon)}
          {item("codex", "ChatGPT / Codex", OpenAiIcon)}
          <div className="my-1.5 mr-10 ml-2">
            <Divider />
          </div>
          {item("claude", "Claude Code", ClaudeIcon)}
          {item("opencode", "OpenCode", OpenCodeIcon)}
          {item("pi", "Pi", PiIcon)}
        </SidebarSection>
        <SidebarSpacer />
        <SidebarSection>
          <UpdateItem state={update} />
        </SidebarSection>
      </SidebarBody>
      <SidebarFooter className="min-h-20 shrink-0 justify-center gap-2">
        <SidebarItem href={`#${settingsPath}`} current={page === "settings"} aria-current={page === "settings" ? "page" : undefined}>
          <SidebarLabel>Настройки</SidebarLabel>
        </SidebarItem>
        <HubAccountMenu hub={account} />
      </SidebarFooter>
    </Sidebar>
  );
}
