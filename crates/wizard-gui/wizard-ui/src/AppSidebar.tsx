import type { ComponentType, ComponentPropsWithoutRef } from "react";
import { useClose } from "@headlessui/react";
import type { AgentId, AgentStates } from "./agents/agents_state";
import type { useProxy } from "./agents/proxy";
import { AppLogo } from "./components/AppLogo";
import { ClaudeIcon, OpenAiIcon, OpenCodeIcon, PiIcon } from "./components/BrandIcons";
import { Badge } from "./components/catalyst/badge";
import { Sidebar, SidebarBody, SidebarFooter, SidebarHeader, SidebarHeading, SidebarItem, SidebarLabel, SidebarSection, SidebarSpacer } from "./components/catalyst/sidebar";
import { OpenLogsItem } from "./logs/OpenLogsButton";
import { type HubAccountControl, HubAccountMenu } from "./settings/HubAccount";
import type { UpdateObservation } from "./updates/update";
import { UpdateItem } from "./updates/UpdateItem";

// Short routing state of an agent, if it is worth showing next to its name.
function AgentBadge({ agents, id, proxy }: { agents: AgentStates; id: AgentId; proxy: ReturnType<typeof useProxy> }) {
  const agent = agents[id];
  if (agent.status === "error") return <Badge color="red" className="ml-auto">Ошибка</Badge>;
  if (agent.status === "not_found") return null;

  const mode = agent.data.routing_mode;
  const chatGptProxyStopped = agents.codex.status === "found" && agents.codex.data.desktop.status === "found"
    && proxy.proxy?.chatgpt === false;
  if (mode === "disabled") return null;
  if (id === "codex" && chatGptProxyStopped) return <Badge color="amber" className="ml-auto">Прокси</Badge>;
  return <Badge color="green" className="ml-auto">{mode === "sub" ? "Подписка" : "Hub"}</Badge>;
}

export function AppSidebar({ page, onSelect, agents, proxy, account, update }: {
  page: AgentId;
  onSelect: (page: AgentId) => void;
  agents: AgentStates | undefined;
  proxy: ReturnType<typeof useProxy>;
  account: HubAccountControl;
  update: UpdateObservation;
}) {
  // Closes the sidebar dialog on narrow windows; does nothing on the desktop layout.
  const close = useClose();

  function item(id: AgentId, label: string, Icon: ComponentType<ComponentPropsWithoutRef<"svg">>) {
    return (
      <SidebarItem current={page === id} aria-current={page === id ? "page" : undefined} onClick={() => { onSelect(id); close(); }}>
        <Icon />
        <SidebarLabel>{label}</SidebarLabel>
        {agents && <AgentBadge agents={agents} id={id} proxy={proxy} />}
      </SidebarItem>
    );
  }

  return (
    <Sidebar>
      <SidebarHeader>
        <div className="px-[calc(--spacing(2)-1px)] py-[calc(--spacing(1.5)-1px)]" role="img" aria-label="CoreInfra Wizard">
          <AppLogo className="h-9 max-w-full" />
        </div>
      </SidebarHeader>
      <SidebarBody>
        <SidebarSection>
          <SidebarHeading className="text-zinc-700 dark:text-zinc-200!">Агенты</SidebarHeading>
          {item("claude_desktop", "Claude Desktop", ClaudeIcon)}
          {item("claude", "Claude Code", ClaudeIcon)}
          {item("codex", "Codex", OpenAiIcon)}
          {item("opencode", "OpenCode", OpenCodeIcon)}
          {item("pi", "Pi", PiIcon)}
        </SidebarSection>
        <SidebarSpacer />
        <SidebarSection>
          <UpdateItem state={update} />
          <OpenLogsItem />
        </SidebarSection>
      </SidebarBody>
      <SidebarFooter>
        <HubAccountMenu hub={account} />
      </SidebarFooter>
    </Sidebar>
  );
}
