import { type ReactNode, useEffect, useRef, useState } from "react";
import {
  type AgentChange,
  type AgentDetection,
  type AgentId,
  type AgentStates,
  type BackupAgent,
  type PreparedAgentChange,
  type TokenChoice,
  prepareAgentChange,
} from "./agents_state";
import {
  routingModes,
  hubModes,
  RoutingFieldset,
  type Hub,
} from "./AgentConfiguration";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { BackupDialog } from "./BackupDialog";
import { DesktopVersion } from "./DesktopInstall";
import { VideoConnection } from "./VideoConnection";
import { MissingAgent } from "./MissingAgent";
import { agentPageNames } from "./navigation";
import { useHubTokens } from "../settings/hub_tokens";
import type { HubAccountControl } from "../settings/HubAccount";
import { useAgentProxyStartup, useProxy } from "./proxy";
import { reportError } from "../logs/log";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import {
  DescriptionDetails,
  DescriptionList,
  DescriptionTerm,
} from "../components/catalyst/description-list";
import { PageHeader, ViewTransition } from "../components/ui/Layout";
import { ErrorText } from "../components/ErrorText";

function Version({
  detection,
}: {
  detection:
    AgentDetection<{ version: string | null }> | { status: "not_found" };
}) {
  if (detection.status === "error")
    return <ErrorText>Не удалось обнаружить: {detection.data}</ErrorText>;
  if (detection.status === "not_found") return <>Не найден</>;
  return (
    <>
      {detection.data.version
        ? `v${detection.data.version}`
        : "Версия неизвестна"}
    </>
  );
}

// Shared daemon state; each page decides what it means for its app.
function ProxyStatus({
  proxy: { proxy, error },
  startup,
}: {
  proxy: ReturnType<typeof useProxy>;
  startup: ReturnType<typeof useAgentProxyStartup>;
}) {
  // Apply or the system-proxy action can recover after an earlier startup failure.
  if (error === undefined && proxy?.enabled)
    return <Badge color="green">Работает</Badge>;
  if (startup.starting) return <Badge color="zinc">Запускается…</Badge>;
  if (startup.error)
    return (
      <div className="space-y-2">
        <ErrorText>{startup.error}</ErrorText>
        <Button plain onClick={startup.retry}>
          Повторить запуск
        </Button>
      </div>
    );
  if (error !== undefined)
    return (
      <Badge color="amber" title={error}>
        Состояние недоступно
      </Badge>
    );
  if (proxy === undefined) return <Badge color="zinc">Загрузка…</Badge>;

  return <Badge color="zinc">Остановлен</Badge>;
}

function BackupButton({
  agent,
  disabled,
}: {
  agent: BackupAgent;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button outline disabled={disabled} onClick={() => setOpen(true)}>
        Резервные копии настроек
      </Button>
      {open && <BackupDialog agent={agent} onClose={() => setOpen(false)} />}
    </>
  );
}

// The selected agent's page. Plan preparation and the plan dialog outlive page switches.
export function AgentPage({
  agents,
  page,
  proxy,
  account,
}: {
  agents: AgentStates;
  page: AgentId | null;
  proxy: ReturnType<typeof useProxy>;
  account: HubAccountControl;
}) {
  const [preparedChange, setPreparedChange] = useState<PreparedAgentChange>();
  const [isPreparingPlan, setIsPreparingPlan] = useState(false);
  const [configurationRevision, setConfigurationRevision] = useState(0);
  const pageContent = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!configurationRevision) return;
    const frame = requestAnimationFrame(() => {
      pageContent.current?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"]')?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [configurationRevision]);
  // Tagged with the page that prepared the change, so it never shows on another agent's page.
  const [error, setError] = useState<{
    page: AgentId | null;
    message: string;
  }>();
  const pageError = error?.page === page ? error.message : undefined;
  const isChangeInProgress = isPreparingPlan || preparedChange !== undefined;
  const hub: Hub = {
    ...useHubTokens(account.account?.active_hub_id),
    signedIn: account.signedIn,
  };
  const proxyAgent =
    page === "codex" &&
    agents.codex.status === "found" &&
    agents.codex.data.desktop.status === "found"
      ? "chatgpt"
      : page === "claude_desktop"
        ? "claude_desktop"
        : null;
  const proxyDetection =
    page === "codex" || page === "claude_desktop" ? agents[page] : undefined;
  const proxyStartup = useAgentProxyStartup(
    proxyDetection?.status === "found" &&
      proxyDetection.data.routing_mode !== "disabled"
      ? proxyAgent
      : null,
  );

  async function prepare(change: AgentChange, token: TokenChoice | null) {
    if (isChangeInProgress) return;

    setIsPreparingPlan(true);
    setError(undefined);

    try {
      setPreparedChange(await prepareAgentChange(change, token));
    } catch (cause: unknown) {
      const message = "Не удалось подготовить изменения";
      reportError(message, cause);
      setError({
        page,
        message: `${message}: ${cause instanceof Error ? cause.message : String(cause)}`,
      });
    } finally {
      setIsPreparingPlan(false);
    }
  }

  let content: ReactNode;
  switch (page) {
    // Keep a prepared plan alive while the settings page is selected.
    case null:
      content = null;
      break;
    case "codex": {
      const codex = agents.codex;
      const chatGptFound =
        codex.status === "found" && codex.data.desktop.status === "found";
      const chatGptProxyStopped =
        !proxyStartup.starting &&
        proxy.proxy?.enabled === false &&
        chatGptFound &&
        codex.status === "found" &&
        codex.data.routing_mode !== "disabled";
      content = (
        <>
          <PageHeader
            title={agentPageNames.codex}
            actions={
              codex.status !== "not_found" && <BackupButton agent="codex" disabled={isChangeInProgress} />
            }
          />
          {codex.status === "error" ? (
            <div className="mt-8">
              <ErrorText>
                Не удалось получить состояние ChatGPT / Codex: {codex.data}
              </ErrorText>
            </div>
          ) : (
            <DescriptionList className="ui-detail-list">
              <DescriptionTerm>ChatGPT</DescriptionTerm>
              <DescriptionDetails>
                <DesktopVersion
                  desktop="chatgpt"
                  installed={chatGptFound}
                  version={codex.status === "found" && codex.data.desktop.status === "found" ? codex.data.desktop.data.version : null}
                  signedIn={account.signedIn}
                  disabled={isChangeInProgress}
                >
                  <Version
                    detection={
                      codex.status === "found" ? codex.data.desktop : codex
                    }
                  />
                </DesktopVersion>
              </DescriptionDetails>
              <DescriptionTerm>Codex CLI</DescriptionTerm>
              <DescriptionDetails>
                <Version
                  detection={codex.status === "found" ? codex.data.cli : codex}
                />
              </DescriptionDetails>
              <DescriptionTerm>Прокси для ChatGPT</DescriptionTerm>
              <DescriptionDetails>
                <ProxyStatus proxy={proxy} startup={proxyStartup} />
              </DescriptionDetails>
            </DescriptionList>
          )}
          {codex.status === "found" && (
            <RoutingFieldset
              key={configurationRevision}
              editing
              needsApply={codex.data.proxy_needs_apply || chatGptProxyStopped}
              agent="codex"
              modes={routingModes("Codex")}
              mode={codex.data.routing_mode}
              toChange={(mode) => ({ CodexSetRouting: mode })}
              agentToken={codex.data.token}
              hub={hub}
              note="Настройка общая для ChatGPT и Codex."
              disabled={isChangeInProgress}
              error={pageError}
              onPrepare={prepare}
            />
          )}
          <VideoConnection hub={hub} hubId={account.account?.active_hub_id} disabled={isChangeInProgress} />
          {(codex.status === "not_found" || (codex.status === "found" && codex.data.cli.status === "not_found")) && (
            <MissingAgent agent="codex" />
          )}
        </>
      );
      break;
    }
    case "claude": {
      const claude = agents.claude;
      content = (
        <>
          <PageHeader
            title={agentPageNames.claude}
            actions={
              claude.status !== "not_found" && <BackupButton agent="claude" disabled={isChangeInProgress} />
            }
          />
          {claude.status === "not_found" ? <MissingAgent agent="claude" /> : <DescriptionList className="ui-detail-list">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails>
              <Version detection={claude} />
            </DescriptionDetails>
          </DescriptionList>}
          {claude.status === "found" && (
            <RoutingFieldset
              key={configurationRevision}
              editing
              agent="claude"
              modes={routingModes("Claude")}
              mode={claude.data.routing_mode}
              toChange={(mode) => ({ ClaudeSetRouting: mode })}
              agentToken={claude.data.token}
              hub={hub}
              disabled={isChangeInProgress}
              error={pageError}
              onPrepare={prepare}
            />
          )}
        </>
      );
      break;
    }
    case "claude_desktop": {
      const desktop = agents.claude_desktop;
      content = (
        <>
          <PageHeader
            title={agentPageNames.claude_desktop}
            actions={
              desktop.status !== "not_found" && <BackupButton
                agent="claude_desktop"
                disabled={isChangeInProgress}
              />
            }
          />
          <DescriptionList className="ui-detail-list">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails>
              <DesktopVersion
                desktop="claude"
                installed={desktop.status === "found"}
                version={desktop.status === "found" ? desktop.data.version : null}
                signedIn={account.signedIn}
                disabled={isChangeInProgress}
              >
                <Version detection={desktop} />
              </DesktopVersion>
            </DescriptionDetails>
            {desktop.status === "found" && (
              <>
                <DescriptionTerm>Прокси</DescriptionTerm>
                <DescriptionDetails>
                  <ProxyStatus proxy={proxy} startup={proxyStartup} />
                </DescriptionDetails>
              </>
            )}
          </DescriptionList>
          {desktop.status === "found" && (
            <RoutingFieldset
              key={configurationRevision}
              editing
              needsApply={
                desktop.data.routing_mode !== "disabled" &&
                (desktop.data.proxy_needs_apply ||
                  (!proxyStartup.starting && proxy.proxy?.enabled === false))
              }
              agent="claude_desktop"
              modes={routingModes("Claude")}
              mode={desktop.data.routing_mode}
              toChange={(mode) => ({ ClaudeDesktopSetRouting: mode })}
              agentToken={desktop.data.token}
              hub={hub}
              disabled={isChangeInProgress}
              error={pageError}
              onPrepare={prepare}
            />
          )}
        </>
      );
      break;
    }
    case "opencode":
    case "pi": {
      const detection = agents[page];
      const name = agentPageNames[page];
      content = (
        <>
          <PageHeader title={name} />
          {detection.status === "not_found" ? <MissingAgent agent={page} /> : <DescriptionList className="ui-detail-list">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails>
              <Version detection={detection} />
            </DescriptionDetails>
          </DescriptionList>}
          {detection.status === "found" && (
            <RoutingFieldset
              key={configurationRevision}
              editing
              agent={page}
              modes={hubModes}
              mode={detection.data.routing_mode}
              toChange={(mode) =>
                page === "opencode"
                  ? { SetOpenCodeHub: mode }
                  : { SetPiHub: mode }
              }
              agentToken={detection.data.token}
              hub={hub}
              disabled={isChangeInProgress}
              error={pageError}
              onPrepare={prepare}
            />
          )}
        </>
      );
      break;
    }
  }

  return (
    <>
      {/* Keyed so that drafts of one agent's form never carry over to another agent. */}
      <div ref={pageContent}><ViewTransition view={page ?? "settings"}>{content}</ViewTransition></div>
      {preparedChange && (
        <PreparedPlanDialog
          prepared={preparedChange}
          onReprepared={setPreparedChange}
          onClose={() => setPreparedChange(undefined)}
          onApplied={() => setConfigurationRevision((value) => value + 1)}
          applyLabel="Применить"
        />
      )}
    </>
  );
}
