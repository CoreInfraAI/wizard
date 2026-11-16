import { type ReactNode, useEffect, useState } from "react";
import { type AgentChange, type AgentId, type HubMode, type RoutingMode, type TokenChoice } from "./agents_state";
import { type HubTokensState, refreshHubTokens } from "../settings/hub_tokens";
import { reportError } from "../logs/log";
import { Button } from "../components/catalyst/button";
import { FieldGroup, Fieldset, Label, Legend } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { useSystemProxy } from "../settings/system_proxy";
import { SystemProxyPrompt } from "../settings/SystemProxyPrompt";

type ModeOption<M extends string> = { mode: M; label: string };

export const currentModeLabels: Record<RoutingMode, string> = {
  sub: "Подписка",
  hub: "API-доступ",
  disabled: "Без CoreInfra",
};

export function routingModes(subscription: "Claude" | "Codex"): ModeOption<RoutingMode>[] {
  return [
    { mode: "hub", label: "CoreInfra AI Hub – API-доступ (оплата за использование)" },
    { mode: "sub", label: `CoreInfra AI Hub – своя подписка ${subscription}` },
    { mode: "disabled", label: "Без CoreInfra AI Hub" },
  ];
}

export const hubModes: ModeOption<HubMode>[] = [
  { mode: "hub", label: "CoreInfra AI Hub – API-доступ (оплата за использование)" },
  { mode: "disabled", label: "Без CoreInfra AI Hub" },
];

// Hub access shared by all pages. Token values are secrets: never log them.
export type Hub = HubTokensState & { signedIn: boolean };

// Names of Hub tokens automatically created for agents.
// Keep in sync with `TokenAgent::token_name` in `crates/wizard-core/src/agents/mod.rs`.
const createdTokenNames: Record<AgentId, string> = {
  codex: "agent-codex",
  claude: "agent-claude-code",
  claude_desktop: "agent-claude-desktop",
  opencode: "agent-opencode",
  pi: "agent-pi",
};

// Only reuse credentials listed in the selected Hub. Wait for its list before
// offering creation; the backend creates on an explicit configure/download action.
export function getAgentToken(agent: AgentId, agentToken: string | null, hub: Hub) {
  const createdName = createdTokenNames[agent];
  const ready = hub.signedIn && hub.status === "ready";
  const existing = hub.tokens.find((token) => token.token === agentToken)
    ?? hub.tokens.filter((token) => token.name === createdName).sort((a, b) => a.issued_at - b.issued_at)[0];
  let selected: TokenChoice | undefined;
  if (ready) {
    selected = existing ? { kind: "existing", token: existing.token } : { kind: "create" };
  }
  return { selected, ready, name: existing ? existing.name || "без названия" : createdName };
}

export function TokenNote({ token, hub, disabled }: {
  token: ReturnType<typeof getAgentToken>;
  hub: Hub;
  disabled: boolean;
}) {
  if (!hub.signedIn) return <Text>Войдите в CoreInfra Hub, чтобы Wizard создал токен для агента.</Text>;
  if (hub.status === "error") return <div className="space-y-2" role="alert">
    <ErrorText>{hub.error}</ErrorText>
    <Button plain disabled={disabled} onClick={() => {
      void refreshHubTokens().catch((cause: unknown) => reportError("Не удалось загрузить токены CoreInfra Hub", cause));
    }}>Повторить</Button>
  </div>;
  if (!token.ready) return <Text role="status">Проверяем токен агента…</Text>;
  return <Text>{token.selected?.kind === "create" ? "Будет создан и использован" : "Будет использован"} токен «{token.name}» для аутентификации в хабе.</Text>;
}

// The same connection form is used by the agent page and initial setup.
export function RoutingFieldset<M extends string>({ agent, modes, mode, toChange, agentToken, token: tokenOverride, hub, note, disabled, error, onPrepare, initialMode, actions, actionsClassName = "" }: {
  agent: AgentId;
  token?: ReturnType<typeof getAgentToken>;
  modes: ModeOption<M>[];
  mode: M;
  toChange: (mode: M) => AgentChange;
  agentToken: string | null;
  hub: Hub;
  note?: ReactNode;
  disabled: boolean;
  error?: string;
  onPrepare: (change: AgentChange, token: TokenChoice | null) => void;
  initialMode?: M;
  actions?: ReactNode;
  actionsClassName?: string;
}) {
  const [draft, setDraft] = useState<M | undefined>(initialMode);
  const selected = draft ?? mode;
  const enabled = selected !== "disabled";
  const proxyOnly = agent === "claude_desktop" && selected === "sub";
  const token = tokenOverride ?? getAgentToken(agent, agentToken, hub);
  const systemProxy = useSystemProxy();
  const [proxyPrompt, setProxyPrompt] = useState(false);

  useEffect(() => {
    if (draft === mode) setDraft(undefined);
  }, [draft, mode]);

  function chooseMode(value: M) {
    setDraft(value);
    if (value === "sub" && systemProxy.state?.status !== "on") setProxyPrompt(true);
  }

  return <>
    <Fieldset className="mt-8">
      <Legend>Подключение</Legend>
      {note && <Text>{note}</Text>}
      <FieldGroup>
        <RadioGroup value={selected} disabled={disabled} aria-label="Режим подключения" onChange={(value) => chooseMode(value as M)}>
          {modes.map((option) => (
            <RadioField key={option.mode} disabled={option.mode !== "disabled" && !hub.signedIn}>
              <Radio value={option.mode} />
              <Label>{option.label}</Label>
            </RadioField>
          ))}
        </RadioGroup>
        {proxyOnly ? <Text>Для аутентификации будет использован токен «coreinfra-ai-wizard» из личного хаба. Если его ещё нет, Wizard создаст его автоматически.</Text>
          : (enabled || tokenOverride !== undefined) && <TokenNote token={token} hub={hub} disabled={disabled} />}
        <div className={`flex flex-wrap items-center gap-3 ${actionsClassName}`}>
          {actions}
          <Button type="button" disabled={disabled || (enabled && (proxyOnly ? !hub.signedIn : !token.ready))}
            onClick={() => onPrepare(toChange(selected), enabled && !proxyOnly ? token.selected ?? null : null)}>
            Завершить настройку
          </Button>
        </div>
        {error && <ErrorText>{error}</ErrorText>}
      </FieldGroup>
    </Fieldset>
    {proxyPrompt && <SystemProxyPrompt observation={systemProxy} onClose={() => setProxyPrompt(false)} />}
  </>;
}
