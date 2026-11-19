import { useEffect, useRef, useState } from "react";
import { type AgentChange, type AgentId, type AgentStates, type PreparedAgentChange, type RoutingMode, type TokenChoice, prepareAgentChange } from "./agents_state";
import { RoutingFieldset, hubModes, routingModes, type Hub } from "./AgentConfiguration";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { MissingAgent } from "./MissingAgent";
import { useProxy } from "./proxy";
import { agentPageNames } from "./navigation";
import { AgentIcon, choices } from "../onboarding/agents";
import { useHubTokens } from "../settings/hub_tokens";
import type { HubAccountControl } from "../settings/HubAccount";
import { Identity } from "../components/ui/Identity";
import { ActionBar } from "../components/ui/Layout";
import { Button } from "../components/catalyst/button";
import { ErrorText } from "../components/ErrorText";

/** The control panel edits the same connection settings without leaving the overview. */
export function AgentConnectionPanel({ agent, agents, account, onBack }: {
  agent: AgentId;
  agents: AgentStates;
  account: HubAccountControl;
  onBack: () => void;
}) {
  const [prepared, setPrepared] = useState<PreparedAgentChange>();
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  const busy = preparing || !!prepared;
  const setAgentBusy = account.setAgentBusy;
  useEffect(() => {
    setAgentBusy(busy);
    return () => setAgentBusy(false);
  }, [busy, setAgentBusy]);
  const hub: Hub = { ...useHubTokens(account.account?.active_hub_id), signedIn: account.signedIn };
  const proxy = useProxy();
  const detection = agents[agent];
  const choice = choices.find(choice => choice.id === agent)!;
  const proxyAgent = agent === "claude_desktop" || (agent === "codex" && agents.codex.status === "found" && agents.codex.data.desktop.status === "found");
  const needsApply = detection.status === "found" && detection.data.routing_mode !== "disabled" &&
    (("proxy_needs_apply" in detection.data && detection.data.proxy_needs_apply) || (proxyAgent && proxy.proxy?.enabled === false));
  const errors = detection.status === "error" ? [detection.data]
    : agent === "codex" && agents.codex.status === "found" ? [agents.codex.data.cli, agents.codex.data.desktop].flatMap(part => part.status === "error" ? [part.data] : []) : [];

  async function prepare(change: AgentChange, token: TokenChoice | null) {
    if (inFlight.current || prepared) return;
    inFlight.current = true;
    setPreparing(true);
    setError(undefined);
    try { setPrepared(await prepareAgentChange(change, token)); }
    catch (cause) { setError(typeof cause === "string" ? cause : "Не удалось подготовить изменения. Попробуйте ещё раз."); }
    finally { inFlight.current = false; setPreparing(false); }
  }
  function toChange(mode: RoutingMode): AgentChange {
    switch (agent) {
      case "codex": return { CodexSetRouting: mode };
      case "claude": return { ClaudeSetRouting: mode };
      case "claude_desktop": return { ClaudeDesktopSetRouting: mode };
      case "opencode": return { SetOpenCodeHub: mode === "hub" ? "hub" : "disabled" };
      case "pi": return { SetPiHub: mode === "hub" ? "hub" : "disabled" };
    }
  }
  return <>
    <Identity icon={<AgentIcon choice={choice} />} title={agentPageNames[agent]} focusable />
    {errors.map(message => <div key={message} className="mt-4" role="alert"><ErrorText>Не удалось обнаружить: {message}</ErrorText></div>)}
    {detection.status === "found" ? <RoutingFieldset
      key={`${agent}:${account.account?.active_hub_id}`}
      editing
      agent={agent} mode={detection.data.routing_mode}
      modes={agent === "pi" || agent === "opencode" ? hubModes : routingModes(agent === "codex" ? "Codex" : "Claude")}
      toChange={toChange} agentToken={detection.data.token} hub={hub}
      needsApply={needsApply} disabled={busy || account.submitting} error={error}
      note={agent === "codex" ? "Настройка общая для ChatGPT и Codex." : undefined}
      actions={<Button plain disabled={busy || account.submitting} onClick={onBack}>Назад</Button>}
      onPrepare={(change, token) => void prepare(change, token)}
    /> : <>
      {detection.status === "not_found" && agent !== "claude_desktop" && <MissingAgent agent={agent} />}
      <ActionBar divider><Button outline onClick={onBack}>Назад</Button></ActionBar>
    </>}
    {prepared && <PreparedPlanDialog prepared={prepared} onReprepared={setPrepared}
      onClose={() => setPrepared(undefined)} onApplied={onBack} applyLabel="Применить" />}
  </>;
}
