import { useEffect, useRef, useState } from "react";
import { hasAgentDetectionError, type AgentId, type useAgentState } from "../agents/agents_state";
import { AgentConnectionPanel } from "../agents/AgentConnectionPanel";
import { agentPageNames, agentPaths } from "../agents/navigation";
import { currentModeLabels } from "../agents/AgentConfiguration";
import { DesktopVersion } from "../agents/DesktopInstall";
import { Button } from "../components/catalyst/button";
import { Badge, BadgeButton } from "../components/catalyst/badge";
import { StateNotice } from "../components/ui/StateNotice";
import { OpenLogsButton } from "../logs/OpenLogsButton";
import { AgentIcon, choices } from "../onboarding/agents";
import type { HubAccountControl } from "../settings/HubAccount";

const overviewChoices = ["claude_desktop", "codex", "claude", "opencode", "pi"].map(
  (id) => choices.find((choice) => choice.id === id)!,
);

export function OverviewAgents({ account, agents }: {
  account: HubAccountControl;
  agents: ReturnType<typeof useAgentState>;
}) {
  const [selected, setSelected] = useState<AgentId>();
  const content = useRef<HTMLDivElement>(null);
  const returnTo = useRef<AgentId | undefined>(undefined);
  useEffect(() => {
    if (selected) content.current?.querySelector<HTMLElement>('[role="radio"][tabindex="0"], h2')?.focus({ preventScroll: true });
    else if (returnTo.current) {
      // Let the nested plan dialog release its focus trap before restoring the row.
      const frame = requestAnimationFrame(() => {
        content.current?.querySelector<HTMLElement>(`[data-agent-status="${returnTo.current}"]`)?.focus({ preventScroll: true });
      });
      return () => cancelAnimationFrame(frame);
    }
  }, [selected]);
  function openAgent(agent: AgentId) { returnTo.current = agent; setSelected(agent); }
  return (
    <div className="overview-agent-manager" ref={content}>
      {selected && agents.status === "ready" ? <AgentConnectionPanel key={selected} agent={selected} agents={agents.data} account={account} onBack={() => setSelected(undefined)} /> :
      <section aria-label="Агенты">
        {agents.status !== "ready" ? (
          <StateNotice tone={agents.status === "loading" ? "loading" : "error"} actions={agents.status === "error" ? <OpenLogsButton /> : undefined}>
            {agents.status === "loading" ? "Ищем установленных агентов…" : "Не удалось проверить приложения. Повторяем проверку автоматически. Подробности доступны в логах."}
          </StateNotice>
        ) : (
          <ul className="overview-agent-list">
            {overviewChoices.map((choice) => {
              const id = choice.agent;
              const detection = agents.data[id];
              const found = detection.status === "found";
              const error = hasAgentDetectionError(detection);
              const needsApply = found && "proxy_needs_apply" in detection.data && detection.data.proxy_needs_apply;
              const name = agentPageNames[id];
              const status = error ? "Не удалось проверить" : !found ? "Не установлен"
                : needsApply ? "Требует настройки" : currentModeLabels[detection.data.routing_mode];
              const desktop = id === "claude_desktop" ? "claude" : id === "codex" ? "chatgpt" : undefined;
              const versionDetection = id === "codex" && agents.data.codex.status === "found"
                ? agents.data.codex.data.desktop : detection;
              const version = versionDetection.status === "found" && "version" in versionDetection.data
                ? versionDetection.data.version : null;
              return (
                <li key={id} className="overview-agent-row">
                  <div className="overview-agent-identity">
                    <AgentIcon choice={choice} />
                    <div className="min-w-0">
                      <a className="overview-agent-name" href={`#${agentPaths[id]}`}>{name}</a>
                      <div className="mt-1">
                        {found ? <BadgeButton data-agent-status={id} color={error || needsApply ? "amber" : detection.data.routing_mode !== "disabled" ? "green" : "zinc"} onClick={() => openAgent(id)} aria-label={`Настройки подключения ${name}: ${status}`}>{status}</BadgeButton>
                          : <Badge color={error ? "amber" : "zinc"}>{status}</Badge>}
                      </div>
                    </div>
                  </div>
                  <div className="overview-agent-actions">
                    {desktop && !error ? (
                      <DesktopVersion desktop={desktop} version={version} installed={versionDetection.status === "found"} signedIn={account.signedIn} disabled={account.agentBusy}>
                        {version ? `v${version}` : versionDetection.status === "found" ? "Версия неизвестна" : "Не установлен"}
                      </DesktopVersion>
                    ) : (
                      <>
                        <span className="overview-agent-version" aria-hidden={version ? undefined : true}>{version ? `v${version}` : ""}</span>
                        {(error || !found) && <Button outline size="sm" data-agent-status={!found ? id : undefined} onClick={() => openAgent(id)} aria-label={`${error ? "Подробнее о" : "Как установить"} ${name}`}>
                          {error ? "Подробнее" : "Как установить"}
                        </Button>}
                      </>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>}
    </div>
  );
}
