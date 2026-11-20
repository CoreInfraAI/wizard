import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Dialog } from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import { ViewTransition } from "../components/ui/Layout";
import { Identity } from "../components/ui/Identity";
import { Field, Label } from "../components/catalyst/fieldset";
import { Select } from "../components/catalyst/select";
import { Text } from "../components/catalyst/text";
import type { HubAccountControl } from "../settings/HubAccount";
import { hubDisplayName } from "../settings/hub_account";
import { useHubTokens } from "../settings/hub_tokens";
import {
  hubModes,
  routingModes,
  RoutingFieldset,
  type Hub,
} from "../agents/AgentConfiguration";
import {
  type ConnectionDraft,
  type ConnectionPlan,
  type useAgentState,
  prepareAgentChange,
} from "../agents/agents_state";
import { PreparedPlanDialog } from "../agents/PreparedPlanDialog";
import { AgentSelection } from "./AgentSelection";
import {
  AgentIcon,
  choices,
  isFound,
  type AgentChoice,
  type ChoiceId,
} from "./agents";
import "./SetupDialog.css";

function Configure({
  choice,
  agents,
  hub,
  account,
  onBusy,
  onComplete,
  onBack,
}: {
  choice: AgentChoice;
  agents: ReturnType<typeof useAgentState>;
  hub: Hub;
  account: HubAccountControl;
  onBusy: (busy: boolean) => void;
  onComplete: () => void;
  onBack: () => void;
}) {
  const [prepared, setPrepared] = useState<ConnectionPlan>();
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  const agent =
    agents.status === "ready" ? agents.data[choice.agent] : undefined;
  const found = agents.status === "ready" && isFound(choice, agents.data);
  const busy = preparing || Boolean(prepared);
  useEffect(() => {
    onBusy(busy);
    return () => onBusy(false);
  }, [busy, onBusy]);

  async function prepare(draft: ConnectionDraft) {
    if (inFlight.current) return;
    inFlight.current = true;
    setPreparing(true);
    setError(undefined);
    try {
      setPrepared(await prepareAgentChange(draft));
    } catch (cause) {
      setError(
        `Не удалось подготовить изменения: ${typeof cause === "string" ? cause : "попробуйте снова"}`,
      );
    } finally {
      inFlight.current = false;
      setPreparing(false);
    }
  }

  return (
    <div className="onboarding-configure">
      <Identity
        icon={<AgentIcon choice={choice} />}
        title={choice.name}
        focusable
      />
      {account.account?.profile && account.account.profile.hubs.length > 1 && (
        <Field className="mt-6">
          <Label>Хаб</Label>
          <Select
            value={account.account.active_hub_id ?? ""}
            disabled={busy || account.submitting}
            onChange={(event) => account.select(event.target.value)}
          >
            {account.account.profile.hubs.map((item) => (
              <option key={item.hub_id} value={item.hub_id}>
                {item.is_personal ? "Личный хаб" : hubDisplayName(item)}
              </option>
            ))}
          </Select>
        </Field>
      )}
      {found && agent?.status === "found" ? (
        <RoutingFieldset
          key={`${choice.id}:${account.account?.active_hub_id}`}
          agent={choice.agent}
          modes={
            choice.agent === "pi" || choice.agent === "opencode"
              ? hubModes
              : routingModes(choice.agent === "codex" ? "Codex" : "Claude")
          }
          mode={agent.data.routing_mode}
          initialMode={
            agent.data.routing_mode === "disabled" ? "hub" : undefined
          }
          agentToken={agent.data.token}
          hub={hub}
          disabled={busy || account.submitting}
          error={error}
          note={
            choice.agent === "codex"
              ? "Настройка общая для ChatGPT и Codex."
              : undefined
          }
          actions={
            <Button plain disabled={busy} onClick={onBack}>
              Назад
            </Button>
          }
          actionsClassName="justify-between"
          toChange={(mode) => {
            switch (choice.agent) {
              case "codex":
                return { CodexSetRouting: mode };
              case "claude":
                return { ClaudeSetRouting: mode };
              case "claude_desktop":
                return { ClaudeDesktopSetRouting: mode };
              case "opencode":
                return { SetOpenCodeHub: mode === "hub" ? "hub" : "disabled" };
              case "pi":
                return { SetPiHub: mode === "hub" ? "hub" : "disabled" };
            }
          }}
          onPrepare={prepare}
        />
      ) : (
        <div className="mt-6 space-y-4">
          <Text>
            {agents.status === "loading"
              ? "Обновляем список приложений…"
              : "Агент не обнаружен. Дождитесь обновления списка или выберите другого агента."}
          </Text>
          <Button plain onClick={onBack}>
            Назад
          </Button>
        </div>
      )}
      {prepared && (
        <PreparedPlanDialog
          prepared={prepared}
          launchChoice={choice}
          onReprepared={setPrepared}
          onClose={() => setPrepared(undefined)}
          onApplied={onComplete}
        />
      )}
    </div>
  );
}

type Stage = "entry" | "select" | "configure";

export function SetupDialog({
  account,
  agents,
  onClose,
}: {
  account: HubAccountControl;
  agents: ReturnType<typeof useAgentState>;
  onClose: () => void;
}) {
  const installed =
    agents.status === "ready"
      ? choices.filter((choice) => isFound(choice, agents.data))
      : [];
  const [selected, setSelected] = useState<ChoiceId>();
  const [stage, setStage] = useState<Stage>("entry");
  // Resolve the entry screen from live detection, not the first (possibly partial) snapshot.
  const viewStage =
    stage === "entry"
      ? installed.length === 1
        ? "configure"
        : "select"
      : stage;
  const [busy, setBusy] = useState(false);
  const content = useRef<HTMLDivElement>(null);
  const hub: Hub = {
    ...useHubTokens(account.account?.active_hub_id),
    signedIn: account.signedIn,
  };
  const selectedChoice =
    stage === "entry" && installed.length === 1
      ? installed[0]
      : choices.find((choice) => choice.id === selected);
  const setAgentBusy = account.setAgentBusy;
  const changeBusy = useCallback(
    (value: boolean) => {
      setBusy(value);
      setAgentBusy(value);
    },
    [setAgentBusy],
  );
  const choose = useCallback((id: ChoiceId) => setSelected(id), []);

  useEffect(() => {
    content.current
      ?.querySelector<HTMLElement>("h2")
      ?.focus({ preventScroll: true });
  }, [viewStage]);
  useEffect(() => () => setAgentBusy(false), [setAgentBusy]);

  function configure(id: ChoiceId) {
    if (!installed.some((choice) => choice.id === id)) return;
    setSelected(id);
    setStage("configure");
  }

  return (
    <Dialog
      open
      onClose={() => {
        if (!busy) onClose();
      }}
      size="2xl"
      className="setup-dialog relative"
    >
      <DialogHeader
        title="Настройка агентов"
        closeLabel="Закрыть настройку"
        disabled={busy}
        onClose={onClose}
      />
      <div ref={content} className="mt-6">
        <ViewTransition view={viewStage}>
          {viewStage === "select" && (
            <AgentSelection
              agents={agents}
              selected={selected}
              onSelect={choose}
              onContinue={configure}
            />
          )}
          {viewStage === "configure" && selectedChoice && (
            <Configure
              choice={selectedChoice}
              agents={agents}
              hub={hub}
              account={account}
              onBusy={changeBusy}
              onComplete={onClose}
              onBack={() => setStage("select")}
            />
          )}
        </ViewTransition>
      </div>
    </Dialog>
  );
}
