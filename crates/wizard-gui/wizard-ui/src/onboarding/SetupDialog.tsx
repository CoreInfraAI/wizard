import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { XMarkIcon } from "@heroicons/react/20/solid";
import { Button } from "../components/catalyst/button";
import { Dialog, DialogTitle } from "../components/catalyst/dialog";
import { Field, Label } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { Select } from "../components/catalyst/select";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import type { HubAccountControl } from "../settings/HubAccount";
import { hubDisplayName } from "../settings/hub_account";
import { useHubTokens } from "../settings/hub_tokens";
import { hubModes, routingModes, RoutingFieldset, type Hub } from "../agents/AgentConfiguration";
import { type AgentChange, type PreparedAgentChange, type TokenChoice, type useAgentState, prepareAgentChange } from "../agents/agents_state";
import { PreparedPlanDialog } from "../agents/PreparedPlanDialog";
import { agentPaths } from "../agents/navigation";
import { AgentSelection, type InstallView } from "./AgentSelection";
import { AgentIcon, choices, isFound, type AgentChoice, type ChoiceId } from "./agents";
import "./SetupDialog.css";

function Configure({ choice, agents, hub, account, onBusy, onComplete, onBack }: {
  choice: AgentChoice;
  agents: ReturnType<typeof useAgentState>;
  hub: Hub;
  account: HubAccountControl;
  onBusy: (busy: boolean) => void;
  onComplete: (restarted: boolean) => void;
  onBack: () => void;
}) {
  const [prepared, setPrepared] = useState<PreparedAgentChange>();
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  const agent = agents.status === "ready" ? agents.data[choice.agent] : undefined;
  const found = agents.status === "ready" && isFound(choice, agents.data);
  const busy = preparing || Boolean(prepared);
  useEffect(() => { onBusy(busy); return () => onBusy(false); }, [busy, onBusy]);

  async function prepare(change: AgentChange, token: TokenChoice | null) {
    if (inFlight.current) return;
    inFlight.current = true;
    setPreparing(true);
    setError(undefined);
    try { setPrepared(await prepareAgentChange(change, token)); }
    catch (cause) { setError(`Не удалось подготовить изменения: ${typeof cause === "string" ? cause : "попробуйте снова"}`); }
    finally { inFlight.current = false; setPreparing(false); }
  }

  return <div className="onboarding-configure">
    <div className="flex items-center gap-3"><span className="onboarding-agent-avatar" data-agent={choice.id}><AgentIcon choice={choice} /></span><Heading level={2} className="font-semibold!" tabIndex={-1}>{choice.name}</Heading></div>
    {account.account?.profile && account.account.profile.hubs.length > 1 && <Field className="mt-6">
      <Label>Хаб</Label>
      <Select value={account.account.active_hub_id ?? ""} disabled={busy || account.submitting} onChange={(event) => account.select(event.target.value)}>
        {account.account.profile.hubs.map((item) => <option key={item.hub_id} value={item.hub_id}>{item.is_personal ? "Личный хаб" : hubDisplayName(item)}</option>)}
      </Select>
    </Field>}
    {found && agent?.status === "found" ? <RoutingFieldset key={`${choice.id}:${account.account?.active_hub_id}`} agent={choice.agent}
      modes={choice.agent === "pi" || choice.agent === "opencode" ? hubModes : routingModes(choice.agent === "codex" ? "Codex" : "Claude")}
      mode={agent.data.routing_mode} initialMode={agent.data.routing_mode === "disabled" ? "hub" : undefined}
      agentToken={agent.data.token} hub={hub} disabled={busy || account.submitting} error={error}
      note={choice.agent === "codex" ? "Настройка общая для ChatGPT и Codex." : undefined}
      actions={<Button plain disabled={busy} onClick={onBack}>Назад</Button>} actionsClassName="justify-between"
      toChange={(mode) => {
        switch (choice.agent) {
          case "codex": return { CodexSetRouting: mode };
          case "claude": return { ClaudeSetRouting: mode };
          case "claude_desktop": return { ClaudeDesktopSetRouting: mode };
          case "opencode": return { SetOpenCodeHub: mode === "hub" ? "hub" : "disabled" };
          case "pi": return { SetPiHub: mode === "hub" ? "hub" : "disabled" };
        }
      }} onPrepare={(change, token) => void prepare(change, token)} /> : <div className="mt-6 space-y-4">
        <Text>{agents.status === "loading" ? "Обновляем список приложений…" : "Агент не обнаружен. Дождитесь обновления списка или выберите другого агента."}</Text>
        <Button plain onClick={onBack}>Назад</Button>
      </div>}
    {prepared && <PreparedPlanDialog prepared={prepared} onReprepared={setPrepared} onClose={() => setPrepared(undefined)} onApplied={() => onComplete(prepared.restarts_desktop)} />}
  </div>;
}

function Completed({ choice, restarted, onFinish, onBusy, onBack }: { choice: AgentChoice; restarted: boolean; onFinish: () => void; onBusy: (busy: boolean) => void; onBack: () => void }) {
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  async function launch() {
    if (inFlight.current) return;
    inFlight.current = true;
    setLaunching(true);
    onBusy(true);
    setError(undefined);
    try {
      await invoke("launch_agent", { agent: choice.id });
      onFinish();
    } catch (cause) {
      setError(typeof cause === "string" ? cause : `Не удалось запустить ${choice.name}. Попробуйте ещё раз.`);
    } finally {
      inFlight.current = false;
      setLaunching(false);
      onBusy(false);
    }
  }
  return <div>
    <div className="flex items-center gap-3">
      <span className="onboarding-agent-avatar" data-agent={choice.id}><AgentIcon choice={choice} /></span>
      <Heading level={2} className="font-semibold!" tabIndex={-1}>{choice.name} настроен</Heading>
    </div>
    <Text className="mt-4">{restarted ? "Приложение открыто с новыми настройками." : choice.desktop ? "Можно открывать приложение и начинать работу." : "Агент откроется в терминале."}</Text>
    <div className="mt-8 flex flex-wrap items-center justify-between gap-3">
      <Button plain disabled={launching} onClick={onBack}>Выбрать другого агента</Button>
      <Button disabled={launching} onClick={() => restarted ? onFinish() : void launch()}>{restarted ? "Готово" : launching ? "Запускаем…" : "Запустить агента"}</Button>
    </div>
    {error && <div className="mt-4" role="alert"><ErrorText>{error}</ErrorText></div>}
  </div>;
}

type Stage = "entry" | "install" | "select" | "configure" | "complete";

export function SetupDialog({ initialStage, account, agents, onClose, onFinished }: {
  initialStage: "install" | "select";
  account: HubAccountControl;
  agents: ReturnType<typeof useAgentState>;
  onClose: () => void;
  onFinished: () => void;
}) {
  const installed = agents.status === "ready" ? choices.filter((choice) => isFound(choice, agents.data)) : [];
  const [selected, setSelected] = useState<ChoiceId>();
  const [stage, setStage] = useState<Stage>(initialStage === "select" ? "entry" : initialStage);
  const [restarted, setRestarted] = useState(false);
  // Resolve the entry screen from live detection, not the first (possibly partial) snapshot.
  const viewStage = stage === "entry" ? installed.length === 1 ? "configure" : "select" : stage;
  const [installView, setInstallView] = useState<InstallView>("choose");
  const [busy, setBusy] = useState(false);
  const content = useRef<HTMLDivElement>(null);
  const hub: Hub = { ...useHubTokens(account.account?.active_hub_id), signedIn: account.signedIn };
  const selectedChoice = stage === "entry" && installed.length === 1 ? installed[0] : choices.find((choice) => choice.id === selected);
  const setAgentBusy = account.setAgentBusy;
  const changeBusy = useCallback((value: boolean) => { setBusy(value); setAgentBusy(value); }, [setAgentBusy]);
  const choose = useCallback((id: ChoiceId) => setSelected(id), []);

  useEffect(() => {
    content.current?.querySelector<HTMLElement>("h2")?.focus({ preventScroll: true });
  }, [viewStage, installView]);
  useEffect(() => () => setAgentBusy(false), [setAgentBusy]);

  function configure(id: ChoiceId) {
    if (!installed.some((choice) => choice.id === id)) return;
    setSelected(id);
    setStage("configure");
  }
  function finish() {
    onFinished();
    onClose();
    if (selectedChoice) window.location.hash = agentPaths[selectedChoice.agent];
  }

  const compact = viewStage === "install" && installView !== "choose";
  return <Dialog open onClose={() => { if (!busy) onClose(); }} size={compact ? "md" : "2xl"} className={`setup-dialog relative${compact ? " setup-dialog-compact" : ""}`}>
    <DialogTitle className="sr-only">{viewStage === "install" ? installView === "confirm" ? "Подтверждение установки" : "Установка агентов" : "Настройка агентов"}</DialogTitle>
    <Button plain className="absolute! top-5 right-5" disabled={busy} aria-label="Закрыть настройку" onClick={onClose}><XMarkIcon /></Button>
    <div ref={content} key={viewStage} className="onboarding-stage">
      {(viewStage === "install" || viewStage === "select") && <AgentSelection mode={viewStage} agents={agents} hub={hub} selected={selected} onSelect={choose} onContinue={configure} onClose={onClose} onInstallViewChange={setInstallView} />}
      {viewStage === "configure" && selectedChoice && <Configure choice={selectedChoice} agents={agents} hub={hub} account={account} onBusy={changeBusy} onComplete={(restarted) => { setSelected(selectedChoice.id); setRestarted(restarted && Boolean(selectedChoice.desktop)); changeBusy(false); setStage("complete"); }} onBack={() => setStage("select")} />}
      {viewStage === "complete" && selectedChoice && <Completed choice={selectedChoice} restarted={restarted} onFinish={finish} onBusy={changeBusy} onBack={() => setStage("select")} />}
    </div>
  </Dialog>;
}
