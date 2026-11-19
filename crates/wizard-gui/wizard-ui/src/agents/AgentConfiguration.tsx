import { type ReactNode, useEffect, useState } from "react";
import { CheckIcon } from "@heroicons/react/20/solid";
import {
  type AgentChange,
  type AgentId,
  type HubMode,
  type RoutingMode,
  type TokenChoice,
} from "./agents_state";
import {
  type HubTokensState,
  refreshHubTokens,
  tokenPrefix,
} from "../settings/hub_tokens";
import { reportError } from "../logs/log";
import { Button } from "../components/catalyst/button";
import {
  Dropdown,
  DropdownButton,
  DropdownDescription,
  DropdownItem,
  DropdownLabel,
  DropdownMenu,
} from "../components/catalyst/dropdown";
import {
  Description,
  FieldGroup,
  Fieldset,
  Label,
} from "../components/catalyst/fieldset";
import { ActionBar, Panel } from "../components/ui/Layout";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { useSystemProxy } from "../settings/system_proxy";
import { SystemProxyPrompt } from "../settings/SystemProxyPrompt";

type ModeOption<M extends string> = {
  mode: M;
  label: string;
  description: string;
};

export const currentModeLabels: Record<RoutingMode, string> = {
  sub: "CoreInfra AI Hub",
  hub: "CoreInfra AI Hub",
  disabled: "Без CoreInfra",
};

export function routingModes(
  subscription: "Claude" | "Codex",
): ModeOption<RoutingMode>[] {
  return [
    {
      mode: "hub",
      label: "API-доступ",
      description: "Через CoreInfra AI Hub, с оплатой за использование.",
    },
    {
      mode: "sub",
      label: `Своя подписка ${subscription}`,
      description: "Подключение через прокси CoreInfra AI Hub.",
    },
    {
      mode: "disabled",
      label: "Без CoreInfra AI Hub",
      description: "Использовать собственные настройки агента.",
    },
  ];
}

export const hubModes: ModeOption<HubMode>[] = [
  {
    mode: "hub",
    label: "API-доступ",
    description: "Через CoreInfra AI Hub, с оплатой за использование.",
  },
  {
    mode: "disabled",
    label: "Без CoreInfra AI Hub",
    description: "Использовать собственные настройки агента.",
  },
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
// offering creation; the backend creates on an explicit configuration action.
export function getAgentToken(
  agent: AgentId,
  agentToken: string | null,
  hub: Hub,
) {
  const createdName = createdTokenNames[agent];
  const ready = hub.signedIn && hub.status === "ready";
  const existing =
    hub.tokens.find((token) => token.token === agentToken) ??
    hub.tokens
      .filter((token) => token.name === createdName)
      .sort((a, b) => a.issued_at - b.issued_at)[0];
  let selected: TokenChoice | undefined;
  if (ready) {
    selected = existing
      ? { kind: "existing", token: existing.token }
      : { kind: "create" };
  }
  return {
    selected,
    ready,
    name: existing ? existing.name || "без названия" : createdName,
  };
}

// Keep a choice local to this agent's configuration form. A token
// must still be listed in the active Hub; saved configuration changes reset it.
export function useAgentToken(
  agent: AgentId,
  agentToken: string | null,
  hub: Hub,
) {
  const [draft, setDraft] = useState<{
    agent: AgentId;
    agentToken: string | null;
    token: string;
  }>();
  const automatic = getAgentToken(agent, agentToken, hub);
  const chosen =
    draft?.agent === agent && draft.agentToken === agentToken
      ? hub.tokens.find((item) => item.token === draft.token)
      : undefined;
  const token =
    automatic.ready && chosen
      ? {
          ...automatic,
          selected: { kind: "existing" as const, token: chosen.token },
          name: chosen.name || "без названия",
        }
      : automatic;
  return {
    ...token,
    automatic,
    choose: (value: string | undefined) =>
      setDraft(
        value === undefined ? undefined : { agent, agentToken, token: value },
      ),
  };
}

export function TokenNote({
  token,
  hub,
  disabled,
}: {
  token: ReturnType<typeof useAgentToken>;
  hub: Hub;
  disabled: boolean;
}) {
  if (!hub.signedIn)
    return (
      <Text>
        Войдите в CoreInfra Hub, чтобы Wizard создал токен для агента.
      </Text>
    );
  if (hub.status === "error")
    return (
      <div className="space-y-2" role="alert">
        <ErrorText>{hub.error}</ErrorText>
        <Button
          plain
          disabled={disabled}
          onClick={() => {
            void refreshHubTokens().catch((cause: unknown) =>
              reportError("Не удалось загрузить токены CoreInfra Hub", cause),
            );
          }}
        >
          Повторить
        </Button>
      </div>
    );
  if (!token.ready) return <Text role="status">Проверяем токен агента…</Text>;
  return (
    <Text>
      {token.selected?.kind === "create"
        ? "Будет создан и использован"
        : "Будет использован"}{" "}
      токен «{token.name}» для аутентификации в хабе.{" "}
      <Dropdown>
        <DropdownButton
          plain
          disabled={disabled}
          aria-label="Изменить токен"
          className="ui-inline-action"
        >
          Изменить
        </DropdownButton>
        <DropdownMenu anchor="bottom start" className="min-w-56 max-w-sm">
          {hub.tokens.map((item) => {
            const selected =
              token.selected?.kind === "existing" &&
              token.selected.token === item.token;
            return (
              <DropdownItem
                key={item.token}
                aria-current={selected ? "true" : undefined}
                disabled={disabled}
                onClick={() => token.choose(item.token)}
              >
                {selected && <CheckIcon />}
                <DropdownLabel className="min-w-0 break-words">
                  {item.name || "Без названия"}
                </DropdownLabel>
                <DropdownDescription>
                  {tokenPrefix(item.token)}
                </DropdownDescription>
              </DropdownItem>
            );
          })}
          {token.automatic.selected?.kind === "create" && (
            <DropdownItem
              aria-current={
                token.selected?.kind === "create" ? "true" : undefined
              }
              disabled={disabled}
              onClick={() => token.choose(undefined)}
            >
              {token.selected?.kind === "create" && <CheckIcon />}
              <DropdownLabel>Создать «{token.automatic.name}»</DropdownLabel>
            </DropdownItem>
          )}
        </DropdownMenu>
      </Dropdown>
    </Text>
  );
}

// The same connection form is used by the agent page and initial setup.
export function RoutingFieldset<M extends string>({
  agent,
  modes,
  mode,
  toChange,
  agentToken,
  hub,
  note,
  disabled,
  error,
  onPrepare,
  initialMode,
  actions,
  actionsClassName = "",
  editing = false,
  needsApply = false,
}: {
  agent: AgentId;
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
  editing?: boolean;
  needsApply?: boolean;
}) {
  const [draft, setDraft] = useState<M | undefined>(initialMode);
  const selected = draft ?? mode;
  const enabled = selected !== "disabled";
  const proxyOnly =
    agent === "claude_desktop" && selected === "sub";
  const token = useAgentToken(agent, agentToken, hub);
  const tokenChanged =
    !proxyOnly &&
    (token.selected?.kind === "create" ||
      (token.selected?.kind === "existing" &&
        token.selected.token !== agentToken));
  const changed =
    selected !== mode || (enabled && (needsApply || tokenChanged));
  const systemProxy = useSystemProxy();
  const [proxyPrompt, setProxyPrompt] = useState(false);

  useEffect(() => {
    if (draft === mode) setDraft(undefined);
  }, [draft, mode]);

  function chooseMode(value: M) {
    setDraft(value);
    if (value === "sub" && systemProxy.state?.status !== "on")
      setProxyPrompt(true);
  }

  return (
    <>
      <Panel title="Подключение" className="mt-6" framed={false}>
        <Fieldset aria-label="Подключение">
          {note && <Text>{note}</Text>}
          <FieldGroup className={note ? "mt-4" : undefined}>
            <RadioGroup
              className="agent-routing-options"
              value={selected}
              disabled={disabled}
              aria-label="Режим подключения"
              onChange={(value) => chooseMode(value as M)}
            >
              {modes.map((option) => (
                <RadioField
                  key={option.mode}
                  className="agent-routing-option"
                  disabled={option.mode !== "disabled" && !hub.signedIn}
                >
                  <Radio value={option.mode} />
                  <Label>{option.label}</Label>
                  <Description>{option.description}</Description>
                </RadioField>
              ))}
            </RadioGroup>
            {proxyOnly ? (
              <Text>Подключение через общий прокси Wizard.</Text>
            ) : (
              enabled && (
                <TokenNote token={token} hub={hub} disabled={disabled} />
              )
            )}
            <ActionBar leading={actions} className={actionsClassName}>
              <Button
                type="button"
                disabled={
                  disabled ||
                  (enabled && (proxyOnly ? !hub.signedIn : !token.ready)) ||
                  (editing && !changed)
                }
                onClick={() =>
                  onPrepare(
                    toChange(selected),
                    enabled && !proxyOnly ? (token.selected ?? null) : null,
                  )
                }
              >
                {editing ? "Применить" : "Завершить настройку"}
              </Button>
            </ActionBar>
            {error && <ErrorText>{error}</ErrorText>}
          </FieldGroup>
        </Fieldset>
      </Panel>
      {proxyPrompt && (
        <SystemProxyPrompt
          observation={systemProxy}
          onClose={() => setProxyPrompt(false)}
        />
      )}
    </>
  );
}
