import { Fragment, type ReactNode, useState } from "react";
import { type AgentChange, type AgentDetection, type AgentId, type AgentStates, type BackupAgent, type PreparedAgentChange, type RoutingMode, type TokenChoice, prepareAgentChange } from "./agents_state";
import { currentModeLabels, routingModes, hubModes, useAgentToken, TokenNote, RoutingFieldset, type Hub } from "./AgentConfiguration";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { BackupDialog } from "./BackupDialog";
import { DesktopInstall, useDesktopInstall } from "./DesktopInstall";
import { tokenLabel, useHubTokens } from "../settings/hub_tokens";
import type { HubAccountControl } from "../settings/HubAccount";
import { useAgentProxyStartup, useProxy } from "./proxy";
import { reportError } from "../logs/log";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import { DescriptionDetails, DescriptionList, DescriptionTerm } from "../components/catalyst/description-list";
import { FieldGroup, Fieldset, Legend } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { ErrorText } from "../components/ErrorText";

function PageHeading({ title, actions }: { title: string; actions?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-4">
      <Heading>{title}</Heading>
      {actions}
    </div>
  );
}

function Version({ detection }: { detection: AgentDetection<{ version: string | null }> | { status: "not_found" } }) {
  if (detection.status === "error") return <ErrorText>Не удалось обнаружить: {detection.data}</ErrorText>;
  if (detection.status === "not_found") return <>Не найден</>;
  return <>{detection.data.version ? `v${detection.data.version}` : "Версия неизвестна"}</>;
}

// Saved routing of an agent: rows of the page's description list.
function RoutingRows({ mode, agentToken, hub }: {
  mode: RoutingMode;
  agentToken: string | null;
  hub: Hub;
}) {
  return (
    <>
      <DescriptionTerm>Режим</DescriptionTerm>
      <DescriptionDetails>
        <Badge color={mode === "disabled" ? "zinc" : "green"}>
          {currentModeLabels[mode]}
        </Badge>
      </DescriptionDetails>
      {mode !== "disabled" && agentToken !== null && (
        <>
          <DescriptionTerm>Токен</DescriptionTerm>
          <DescriptionDetails>{tokenLabel(agentToken, hub.tokens)}</DescriptionDetails>
        </>
      )}
    </>
  );
}

// Shared daemon state; each page decides what it means for its app.
function ProxyStatus({ proxy: { proxy, error }, startup }: {
  proxy: ReturnType<typeof useProxy>;
  startup: ReturnType<typeof useAgentProxyStartup>;
}) {
  // Apply or the system-proxy action can recover after an earlier startup failure.
  if (error === undefined && proxy?.enabled) return <Badge color="green">Работает</Badge>;
  if (startup.starting) return <Badge color="zinc">Запускается…</Badge>;
  if (startup.error) return <div className="space-y-2">
    <ErrorText>{startup.error}</ErrorText>
    <Button plain onClick={startup.retry}>Повторить запуск</Button>
  </div>;
  if (error !== undefined) return <Badge color="amber" title={error}>Состояние недоступно</Badge>;
  if (proxy === undefined) return <Badge color="zinc">Загрузка…</Badge>;

  return <Badge color="zinc">Остановлен</Badge>;
}

function BackupButton({ agent, disabled }: { agent: BackupAgent; disabled: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button outline disabled={disabled} onClick={() => setOpen(true)}>Резервные копии настроек</Button>
      {open && <BackupDialog agent={agent} onClose={() => setOpen(false)} />}
    </>
  );
}

// The selected agent's page. Plan preparation and the plan dialog outlive page switches.
export function AgentPage({ agents, page, proxy, account }: {
  agents: AgentStates;
  page: AgentId | null;
  proxy: ReturnType<typeof useProxy>;
  account: HubAccountControl;
}) {
  const [preparedChange, setPreparedChange] = useState<PreparedAgentChange>();
  const [isPreparingPlan, setIsPreparingPlan] = useState(false);
  // Tagged with the page that prepared the change, so it never shows on another agent's page.
  const [error, setError] = useState<{ page: AgentId | null; message: string }>();
  const pageError = error?.page === page ? error.message : undefined;
  const isChangeInProgress = isPreparingPlan || preparedChange !== undefined;
  const hub: Hub = {
    ...useHubTokens(account.account?.active_hub_id),
    signedIn: account.signedIn,
  };
  const claudeDesktopToken = useAgentToken("claude_desktop", agents.claude_desktop.status === "found" ? agents.claude_desktop.data.token : null, hub);
  const claudeInstall = useDesktopInstall("claude");
  // ChatGPT downloads use the token of the shared ChatGPT / Codex routing.
  const codexToken = useAgentToken("codex", agents.codex.status === "found" ? agents.codex.data.token : null, hub);
  const chatGptInstall = useDesktopInstall("chatgpt");
  const proxyAgent = page === "codex" || page === "claude_desktop" ? page : null;
  const proxyDetection = proxyAgent ? agents[proxyAgent] : undefined;
  const proxyStartup = useAgentProxyStartup(proxyDetection?.status === "found" && proxyDetection.data.routing_mode !== "disabled" ? proxyAgent : null);

  async function prepare(change: AgentChange, token: TokenChoice | null) {
    if (isChangeInProgress) return;

    setIsPreparingPlan(true);
    setError(undefined);

    try {
      setPreparedChange(await prepareAgentChange(change, token));
    } catch (cause: unknown) {
      const message = "Не удалось подготовить изменения";
      reportError(message, cause);
      setError({ page, message: `${message}: ${cause instanceof Error ? cause.message : String(cause)}` });
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
      const chatGptFound = codex.status === "found" && codex.data.desktop.status === "found";
      const chatGptProxyStopped = !proxyStartup.starting && proxy.proxy?.enabled === false && chatGptFound
        && codex.status === "found" && codex.data.routing_mode !== "disabled";
      content = (
        <>
          <PageHeading title="ChatGPT / Codex" actions={<BackupButton agent="codex" disabled={isChangeInProgress} />} />
          {codex.status === "error" ? (
            <div className="mt-8"><ErrorText>Не удалось получить состояние ChatGPT / Codex: {codex.data}</ErrorText></div>
          ) : (
            <DescriptionList className="mt-8 sm:mt-10">
              <DescriptionTerm>ChatGPT</DescriptionTerm>
              <DescriptionDetails><Version detection={codex.status === "found" ? codex.data.desktop : codex} /></DescriptionDetails>
              <DescriptionTerm>Codex CLI</DescriptionTerm>
              <DescriptionDetails><Version detection={codex.status === "found" ? codex.data.cli : codex} /></DescriptionDetails>
              <DescriptionTerm>Прокси для ChatGPT</DescriptionTerm>
              <DescriptionDetails>
                <ProxyStatus proxy={proxy} startup={proxyStartup} />
              </DescriptionDetails>
              {codex.status === "found" && (
                <RoutingRows mode={codex.data.routing_mode} agentToken={codex.data.token} hub={hub} />
              )}
            </DescriptionList>
          )}
          {codex.status === "found" && (
            <RoutingFieldset editing needsApply={chatGptProxyStopped} agent="codex" token={chatGptInstall.hidden ? undefined : codexToken} modes={routingModes("Codex")} mode={codex.data.routing_mode}
              toChange={(mode) => ({ CodexSetRouting: mode })} agentToken={codex.data.token} hub={hub}
              note="Настройка общая для ChatGPT и Codex." disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
          )}
          {!chatGptInstall.hidden && (
            <Fieldset className="mt-10">
              <Legend>{chatGptFound ? "Обновление ChatGPT" : "Установка ChatGPT"}</Legend>
              <FieldGroup>
                {codex.status !== "found" && <TokenNote token={codexToken} hub={hub} disabled={isChangeInProgress} />}
                <DesktopInstall observation={chatGptInstall} token={codexToken.selected} tokenReady={codexToken.ready}
                  disabled={isChangeInProgress} installed={chatGptFound} />
              </FieldGroup>
            </Fieldset>
          )}
        </>
      );
      break;
    }
    case "claude": {
      const claude = agents.claude;
      content = (
        <>
          <PageHeading title="Claude Code" actions={<BackupButton agent="claude" disabled={isChangeInProgress} />} />
          <DescriptionList className="mt-8 sm:mt-10">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails><Version detection={claude} /></DescriptionDetails>
            {claude.status === "found" && (
              <RoutingRows mode={claude.data.routing_mode} agentToken={claude.data.token} hub={hub} />
            )}
          </DescriptionList>
          {claude.status === "found" && (
            <RoutingFieldset editing agent="claude" modes={routingModes("Claude")} mode={claude.data.routing_mode}
              toChange={(mode) => ({ ClaudeSetRouting: mode })} agentToken={claude.data.token} hub={hub}
              disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
          )}
        </>
      );
      break;
    }
    case "claude_desktop": {
      const desktop = agents.claude_desktop;
      content = (
        <>
          <PageHeading title="Claude Desktop" actions={<BackupButton agent="claude_desktop" disabled={isChangeInProgress} />} />
          <DescriptionList className="mt-8 sm:mt-10">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails><Version detection={desktop} /></DescriptionDetails>
            {desktop.status === "found" && (
              <>
                <DescriptionTerm>Прокси</DescriptionTerm>
                <DescriptionDetails><ProxyStatus proxy={proxy} startup={proxyStartup} /></DescriptionDetails>
                <RoutingRows mode={desktop.data.routing_mode} agentToken={desktop.data.token} hub={hub} />
              </>
            )}
          </DescriptionList>
          {desktop.status === "found" && (
            <RoutingFieldset editing needsApply={desktop.data.proxy_needs_apply || (!proxyStartup.starting && proxy.proxy?.enabled === false)} agent="claude_desktop" token={claudeDesktopToken} modes={routingModes("Claude")} mode={desktop.data.routing_mode}
              toChange={(mode) => ({ ClaudeDesktopSetRouting: mode })} agentToken={desktop.data.token} hub={hub}
              disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
          )}
          <Fieldset className="mt-10">
            <Legend>{desktop.status === "found" ? "Обновление приложения" : "Установка приложения"}</Legend>
            <FieldGroup>
              {(desktop.status !== "found" || desktop.data.routing_mode === "sub") && <TokenNote token={claudeDesktopToken} hub={hub} disabled={isChangeInProgress} />}
              <DesktopInstall observation={claudeInstall} token={claudeDesktopToken.selected} tokenReady={claudeDesktopToken.ready}
                disabled={isChangeInProgress} installed={desktop.status === "found"} />
            </FieldGroup>
          </Fieldset>
        </>
      );
      break;
    }
    case "opencode":
    case "pi": {
      const detection = agents[page];
      const name = page === "opencode" ? "OpenCode" : "Pi";
      content = (
        <>
          <PageHeading title={name} />
          <DescriptionList className="mt-8 sm:mt-10">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails><Version detection={detection} /></DescriptionDetails>
            {detection.status === "found" && (
              <RoutingRows mode={detection.data.routing_mode} agentToken={detection.data.token} hub={hub} />
            )}
          </DescriptionList>
          {detection.status === "found" && (
            <RoutingFieldset editing agent={page} modes={hubModes} mode={detection.data.routing_mode}
              toChange={(mode) => (page === "opencode" ? { SetOpenCodeHub: mode } : { SetPiHub: mode })}
              agentToken={detection.data.token} hub={hub} disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
          )}
        </>
      );
      break;
    }
  }

  return (
    <>
      {/* Keyed so that drafts of one agent's form never carry over to another agent. */}
      <Fragment key={page}>{content}</Fragment>
      {preparedChange && <PreparedPlanDialog prepared={preparedChange} onReprepared={setPreparedChange} onClose={() => setPreparedChange(undefined)} applyLabel="Применить" />}
    </>
  );
}
