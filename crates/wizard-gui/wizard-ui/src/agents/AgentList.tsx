import { type ReactNode, useEffect, useState } from "react";
import { type AgentChange, type AgentDetection, type AgentId, type AgentStates, type BackupAgent, type HubMode, type PreparedAgentChange, type RoutingMode, type TokenChoice, prepareAgentChange } from "./agents_state";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { BackupDialog } from "./BackupDialog";
import { type HubTokensState, tokenLabel, tokenPrefix, useHubTokens } from "../settings/hub_tokens";
import { useHubAccount } from "../settings/hub_account";
import { type Proxy, useProxy } from "./proxy";
import { reportError } from "../logs/log";
import { Button } from "../components/catalyst/button";
import { Badge } from "../components/catalyst/badge";
import { Heading } from "../components/catalyst/heading";
import { Label } from "../components/catalyst/fieldset";
import { Listbox, ListboxLabel, ListboxOption } from "../components/catalyst/listbox";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

type ModeOption<M extends string> = { mode: M; label: string; detail?: string };

function routingModes(subscription: string): ModeOption<RoutingMode>[] {
  return [
    { mode: "disabled", label: "Без CoreInfra" },
    { mode: "hub", label: "CoreInfra Hub" },
    { mode: "sub", label: "CoreInfra Hub", detail: ` — через вашу подписку ${subscription}` },
  ];
}

const hubModes: ModeOption<HubMode>[] = [
  { mode: "disabled", label: "Без CoreInfra" },
  { mode: "hub", label: "CoreInfra Hub" },
];

// Hub access shared by all tiles. Token values are secrets: never log them.
type Hub = HubTokensState & { signedIn: boolean };

// Names of Hub tokens created for agents, used for the default choice and the create option.
// Keep in sync with `TokenAgent::token_name` in `crates/wizard-core/src/agents/mod.rs`.
const createdTokenNames: Record<AgentId, string> = {
  codex: "agent-codex",
  claude: "agent-claude-code",
  claude_desktop: "agent-claude-desktop",
  opencode: "agent-opencode",
  pi: "agent-pi",
};

// `token: null` is the option that creates the agent's Hub token.
type TokenOption = { key: string; label: string; token: string | null };

// Everything the token menu shows. Choosing an option only changes the local draft: creating
// happens on «Применить», in the backend, before the plan is prepared.
// Selected: the draft, else the agent's token, else the oldest Hub token with the created name,
// else creating. Creating is offered only when the loaded list has no such token: an unknown list
// must not suggest a duplicate. Once a created token is listed, it replaces a create draft
// (not the agent's other token), so a retry after a failed preparation does not create another.
function useTokenMenu(agent: AgentId, agentToken: string | null, hub: Hub) {
  const [draft, setDraft] = useState<TokenChoice>();
  const createdName = createdTokenNames[agent];

  const options: TokenOption[] = [];
  // The agent's own token comes first, even when Hub does not list it (another hub, signed out).
  if (agentToken !== null) {
    options.push({ key: agentToken, label: `${tokenLabel(agentToken, hub.tokens)} (Текущий)`, token: agentToken });
  }
  // Other Hub tokens, without the current one and tokens Wizard created for other agents;
  // this agent's created tokens come first.
  const otherNames = Object.values(createdTokenNames).filter((name) => name !== createdName);
  const visible = hub.tokens
    .filter((token) => token.token !== agentToken && !otherNames.includes(token.name))
    .sort((a, b) => Number(b.name === createdName) - Number(a.name === createdName));
  for (const token of visible) {
    options.push({ key: token.token, label: token.name || tokenPrefix(token.token), token: token.token });
  }
  const listed = (token: string | null | undefined): token is string =>
    options.some((option) => option.token !== null && option.token === token);

  const named = hub.tokens
    .filter((token) => token.name === createdName)
    .sort((a, b) => a.issued_at - b.issued_at)[0]?.token;
  const canCreate = hub.signedIn && hub.status === "ready" && named === undefined;
  if (canCreate) {
    options.splice(1, 0, { key: "create", label: `Создать ${createdName}`, token: null });
  }

  let selected: TokenChoice | undefined;
  if (draft?.kind === "create") {
    selected = named === undefined ? draft : { kind: "existing", token: named };
  } else {
    const token = [draft?.token, agentToken, named].find(listed);
    selected = token !== undefined ? { kind: "existing", token } : canCreate ? { kind: "create" } : undefined;
  }
  const ready = hub.signedIn && hub.status === "ready" && selected !== undefined
    && (selected.kind === "create" ? canCreate : listed(selected.token));
  const selectedOption = options.find((option) =>
    selected?.kind === "create" ? option.token === null : option.token === selected?.token);

  // An applied change replaces the agent token; show it instead of the old draft.
  useEffect(() => setDraft(undefined), [agentToken]);

  function choose(option: TokenOption) {
    setDraft(option.token === null ? { kind: "create" } : { kind: "existing", token: option.token });
  }

  return { options, selected, selectedOption, choose, ready };
}

function TokenSelect({ options, value, disabled, onChange }: {
  options: TokenOption[];
  value: TokenOption | undefined;
  disabled: boolean;
  onChange: (option: TokenOption) => void;
}) {
  // The «Токен ·» prefix names the field on the closed menu only: options render in a portal
  // outside the button.
  return (
    <Listbox value={value} by="key" aria-label="Токен CoreInfra Hub" placeholder="Выберите токен"
      disabled={disabled} onChange={onChange}>
      {options.map((option) => (
        <ListboxOption key={option.key} value={option}>
          <ListboxLabel>
            <span className="hidden text-zinc-500 in-[button]:inline dark:text-zinc-400">Токен · </span>
            {option.label}
          </ListboxLabel>
        </ListboxOption>
      ))}
    </Listbox>
  );
}

// Mode and token of one agent. The saved mode wins until the user picks another one.
function RoutingSelector<M extends string>({ agent, modes, mode, toChange, agentToken, hub, badges, disabled, onPrepare }: {
  agent: AgentId;
  modes: ModeOption<M>[];
  mode: M;
  toChange: (mode: M) => AgentChange;
  agentToken: string | null;
  hub: Hub;
  badges?: ReactNode;
  disabled: boolean;
  onPrepare: (change: AgentChange, token: TokenChoice | null) => void;
}) {
  const [draft, setDraft] = useState<M>();
  const selected = draft ?? mode;
  const enabled = selected !== "disabled";
  const token = useTokenMenu(agent, agentToken, hub);

  useEffect(() => {
    if (draft === mode) setDraft(undefined);
  }, [draft, mode]);

  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Badge color={mode === "disabled" ? "zinc" : "green"}>
          {modes.find((option) => option.mode === mode)?.label}
          {mode !== "disabled" && " · активно"}
          {mode !== "disabled" && agentToken !== null && ` · ${tokenLabel(agentToken, hub.tokens)}`}
        </Badge>
        {badges}
      </div>
      <RadioGroup value={selected} disabled={disabled} onChange={(value) => setDraft(value as M)}>
        {modes.map((option) => (
          <RadioField key={option.mode} disabled={option.mode !== "disabled" && !hub.signedIn}>
            <Radio value={option.mode} />
            <Label>{option.label}{option.detail}</Label>
          </RadioField>
        ))}
      </RadioGroup>
      {enabled && (
        <div>
          <TokenSelect options={token.options} value={token.selectedOption} disabled={disabled || !hub.signedIn} onChange={token.choose} />
          {hub.status === "error" && <ErrorText>{hub.error}</ErrorText>}
        </div>
      )}
      <Button outline type="button" disabled={disabled || (enabled && !token.ready)}
        onClick={() => onPrepare(toChange(selected), enabled ? token.selected ?? null : null)}>
        Применить
      </Button>
    </>
  );
}

function Installation({ name, detection, children, actions }: {
  name: string;
  detection: AgentDetection<{ path: string; version: string | null }>;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="flex min-w-0 flex-col gap-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
          <Heading level={2}>{name}</Heading>
          {detection.status === "found" && (
            <span className="text-sm text-zinc-500 dark:text-zinc-400">
              {detection.data.version ? `v${detection.data.version}` : "Версия неизвестна"}
            </span>
          )}
        </div>
        {actions && <div className="max-w-1/2 shrink-0">{actions}</div>}
      </div>
      {detection.status === "found" && (
        <>
          {children && <div className="space-y-3">{children}</div>}
{/*
          <details className="mt-auto border-t border-zinc-200 pt-3 text-sm/6 text-zinc-500 dark:border-zinc-800 dark:text-zinc-400">
            <summary className="cursor-pointer">Сведения об установке</summary>
            <p className="mt-2">
              Путь: <code className="select-text break-all text-zinc-700 dark:text-zinc-300">
                {detection.data.path}
              </code>
            </p>
          </details>
*/}
        </>
      )}
      {detection.status === "not_found" && <Text>Не найден</Text>}
      {detection.status === "error" && (
        <ErrorText>Не удалось обнаружить {name}: {detection.data}</ErrorText>
      )}
    </section>
  );
}

function InstallationStatus({ name, detection, children }: {
  name: string;
  detection?: AgentDetection<{ path: string; version: string | null }>;
  children?: ReactNode;
}) {
  return (
    <div>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <Heading level={2}>{name}</Heading>
        <span className="text-sm text-zinc-500 dark:text-zinc-400">
          {detection?.status === "found"
            ? detection.data.version ? `v${detection.data.version}` : "Версия неизвестна"
            : detection?.status === "not_found" ? "Не найден" : "Статус неизвестен"}
        </span>
        {children}
      </div>
      {detection?.status === "error" && <ErrorText>{detection.data}</ErrorText>}
    </div>
  );
}

function BackupButton({ agent, disabled }: { agent: BackupAgent; disabled: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button plain disabled={disabled} onClick={() => setOpen(true)}>Резервные копии…</Button>
      {open && <BackupDialog agent={agent} onClose={() => setOpen(false)} />}
    </>
  );
}

// State of the app's listener only; each tile decides what it means for its app.
function ProxyStatus({ proxy: { proxy, error }, app }: { proxy: ReturnType<typeof useProxy>; app: keyof Proxy }) {
  if (error !== undefined) return <Badge color="amber" title={error}>Прокси: состояние недоступно</Badge>;
  if (proxy === undefined) return <Badge color="zinc">Прокси: загрузка…</Badge>;

  const running = proxy[app];
  return (
    <Badge color={running ? "green" : "zinc"}>
      {running ? "Прокси работает" : "Прокси остановлен"}
    </Badge>
  );
}

export function AgentList({ agents }: { agents: AgentStates }) {
  const [preparedChange, setPreparedChange] = useState<PreparedAgentChange>();
  const [isPreparingPlan, setIsPreparingPlan] = useState(false);
  const [error, setError] = useState<string>();
  const isChangeInProgress = isPreparingPlan || preparedChange !== undefined;
  const { account } = useHubAccount();
  const hub: Hub = { ...useHubTokens(), signedIn: account?.signed_in === true };
  const proxy = useProxy();
  const codex = agents.codex;
  const chatGptProxyStopped = proxy.proxy?.chatgpt === false && codex.status === "found"
    && codex.data.desktop.status === "found" && codex.data.routing_mode !== "disabled";

  async function prepare(change: AgentChange, token: TokenChoice | null) {
    if (isChangeInProgress) return;

    setIsPreparingPlan(true);
    setError(undefined);

    try {
      setPreparedChange(await prepareAgentChange(change, token));
    } catch (cause: unknown) {
      const message = "Не удалось подготовить изменения";
      reportError(message, cause);
      setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setIsPreparingPlan(false);
    }
  }

  return (
    <>
      <section className="flex min-w-0 flex-col gap-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-1">
            <InstallationStatus name="Codex" detection={agents.codex.status === "found"
              ? agents.codex.data.cli : agents.codex.status === "not_found" ? agents.codex : undefined} />
            <InstallationStatus name="ChatGPT" detection={agents.codex.status === "found"
              ? agents.codex.data.desktop : agents.codex.status === "not_found" ? agents.codex : undefined}>
              <ProxyStatus proxy={proxy} app="chatgpt" />
            </InstallationStatus>
            {chatGptProxyStopped && <ErrorText>Прокси для ChatGPT не запущен, нажмите применить</ErrorText>}
          </div>
          <div className="max-w-1/2 shrink-0"><BackupButton agent="codex" disabled={isChangeInProgress} /></div>
        </div>
        <Text>Настройка общая для Codex и ChatGPT.</Text>
        {agents.codex.status === "found" && (
          <div className="space-y-3 border-t border-zinc-200 pt-3 dark:border-zinc-800">
            <RoutingSelector agent="codex" modes={routingModes("ChatGPT")} mode={agents.codex.data.routing_mode}
              toChange={(mode) => ({ CodexSetRouting: mode })} agentToken={agents.codex.data.token} hub={hub}
              disabled={isChangeInProgress} onPrepare={prepare} />
          </div>
        )}
        {agents.codex.status === "error" && (
          <ErrorText>Не удалось получить состояние Codex / ChatGPT: {agents.codex.data}</ErrorText>
        )}
      </section>
      <Installation name="Claude Code" detection={agents.claude} actions={<BackupButton agent="claude" disabled={isChangeInProgress} />}>
        {agents.claude.status === "found" && (
          <RoutingSelector agent="claude" modes={routingModes("Claude")} mode={agents.claude.data.routing_mode}
            toChange={(mode) => ({ ClaudeSetRouting: mode })} agentToken={agents.claude.data.token} hub={hub}
            disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      <Installation name="Claude Desktop" detection={agents.claude_desktop}>
        {agents.claude_desktop.status === "found" && (
          <RoutingSelector agent="claude_desktop" modes={routingModes("Claude")} mode={agents.claude_desktop.data.routing_mode}
            toChange={(mode) => ({ ClaudeDesktopSetRouting: mode })} agentToken={agents.claude_desktop.data.token} hub={hub}
            badges={<ProxyStatus proxy={proxy} app="claude_desktop" />} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
        <Text>Для входа в аккаунт включите VPN и примените режим «Без CoreInfra» в Wizard.</Text>
      </Installation>
      <Installation name="OpenCode" detection={agents.opencode}>
        {agents.opencode.status === "found" && (
          <RoutingSelector agent="opencode" modes={hubModes} mode={agents.opencode.data.routing_mode}
            toChange={(mode) => ({ SetOpenCodeHub: mode })} agentToken={agents.opencode.data.token} hub={hub}
            disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      <Installation name="Pi" detection={agents.pi}>
        {agents.pi.status === "found" && (
          <RoutingSelector agent="pi" modes={hubModes} mode={agents.pi.data.routing_mode}
            toChange={(mode) => ({ SetPiHub: mode })} agentToken={agents.pi.data.token} hub={hub}
            disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      {error && <div className="col-span-full"><ErrorText>{error}</ErrorText></div>}
      {preparedChange && <PreparedPlanDialog prepared={preparedChange} onClose={() => setPreparedChange(undefined)} />}
    </>
  );
}
