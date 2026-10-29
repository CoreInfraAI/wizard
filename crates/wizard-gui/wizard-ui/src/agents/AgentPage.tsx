import { Fragment, type ReactNode, useEffect, useState } from "react";
import { type AgentChange, type AgentDetection, type AgentId, type AgentStates, type BackupAgent, type HubMode, type PreparedAgentChange, type RoutingMode, type TokenChoice, prepareAgentChange } from "./agents_state";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { BackupDialog } from "./BackupDialog";
import { ClaudeDesktopInstall, useClaudeDesktopInstall } from "./ClaudeDesktopInstall";
import { type HubTokensState, refreshHubTokens, tokenLabel, tokenPrefix, useHubTokens } from "../settings/hub_tokens";
import type { HubAccountControl } from "../settings/HubAccount";
import { type Proxy, useProxy } from "./proxy";
import { reportError } from "../logs/log";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import { DescriptionDetails, DescriptionList, DescriptionTerm } from "../components/catalyst/description-list";
import { Description, ErrorMessage, Field, FieldGroup, Fieldset, Label, Legend } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { Listbox, ListboxDescription, ListboxLabel, ListboxOption } from "../components/catalyst/listbox";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

type ModeOption<M extends string> = { mode: M; label: string };

function routingModes(subscription: string): ModeOption<RoutingMode>[] {
  return [
    { mode: "disabled", label: "Без CoreInfra" },
    { mode: "hub", label: "CoreInfra Hub" },
    { mode: "sub", label: `CoreInfra Hub — через вашу подписку ${subscription}` },
  ];
}

const hubModes: ModeOption<HubMode>[] = [
  { mode: "disabled", label: "Без CoreInfra" },
  { mode: "hub", label: "CoreInfra Hub" },
];

// Hub access shared by all pages. Token values are secrets: never log them.
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
type TokenOption = { key: string; label: string; current?: boolean; token: string | null };

// Everything the token menu shows. Choosing an option only changes the local draft: creating
// happens on an explicit apply/check/download action in the backend.
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
    options.push({ key: agentToken, label: tokenLabel(agentToken, hub.tokens), current: true, token: agentToken });
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

function TokenField({ token, hub, disabled }: {
  token: ReturnType<typeof useTokenMenu>;
  hub: Hub;
  disabled: boolean;
}) {
  return (
    <Field>
      <Label>Токен CoreInfra Hub</Label>
      <Listbox disabled={disabled || !hub.signedIn} value={token.selectedOption} by="key" placeholder="Выберите токен" onChange={token.choose} className="max-w-sm">
        {token.options.map((option) => (
          <ListboxOption key={option.key} value={option}>
            <ListboxLabel>{option.label}</ListboxLabel>
            {option.current && <ListboxDescription>Текущий</ListboxDescription>}
          </ListboxOption>
        ))}
      </Listbox>
      {!hub.signedIn && <Description>Войдите в CoreInfra Hub для выбора ключа.</Description>}
      {hub.status === "error" && (
        <ErrorMessage>
          {hub.error}{" "}
          <button type="button" className="underline" onClick={() => {
            void refreshHubTokens().catch((cause: unknown) => reportError("Не удалось загрузить токены CoreInfra Hub", cause));
          }}>
            Повторить
          </button>
        </ErrorMessage>
      )}
    </Field>
  );
}

// Mode and token of one agent. The saved mode wins until the user picks another one.
function RoutingFieldset<M extends string>({ agent, modes, mode, toChange, agentToken, token: tokenOverride, hub, note, disabled, error, onPrepare }: {
  agent: AgentId;
  token?: ReturnType<typeof useTokenMenu>;
  modes: ModeOption<M>[];
  mode: M;
  toChange: (mode: M) => AgentChange;
  agentToken: string | null;
  hub: Hub;
  note?: ReactNode;
  disabled: boolean;
  error?: string;
  onPrepare: (change: AgentChange, token: TokenChoice | null) => void;
}) {
  const [draft, setDraft] = useState<M>();
  const selected = draft ?? mode;
  const enabled = selected !== "disabled";
  const localToken = useTokenMenu(agent, agentToken, hub);
  const token = tokenOverride ?? localToken;

  useEffect(() => {
    if (draft === mode) setDraft(undefined);
  }, [draft, mode]);

  return (
    <Fieldset className="mt-10">
      <Legend>Подключение</Legend>
      {note && <Text>{note}</Text>}
      <FieldGroup>
        <RadioGroup value={selected} disabled={disabled} aria-label="Режим" onChange={(value) => setDraft(value as M)}>
          {modes.map((option) => (
            <RadioField key={option.mode} disabled={option.mode !== "disabled" && !hub.signedIn}>
              <Radio value={option.mode} />
              <Label>{option.label}</Label>
            </RadioField>
          ))}
        </RadioGroup>
        {(enabled || tokenOverride !== undefined) && <TokenField token={token} hub={hub} disabled={disabled} />}
        <div>
          <Button type="button" disabled={disabled || (enabled && !token.ready)}
            onClick={() => onPrepare(toChange(selected), enabled ? token.selected ?? null : null)}>
            Применить
          </Button>
          {error && <div className="mt-3"><ErrorText>{error}</ErrorText></div>}
        </div>
      </FieldGroup>
    </Fieldset>
  );
}

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
function RoutingRows({ modes, mode, agentToken, hub }: {
  modes: ModeOption<string>[];
  mode: string;
  agentToken: string | null;
  hub: Hub;
}) {
  return (
    <>
      <DescriptionTerm>Режим</DescriptionTerm>
      <DescriptionDetails>
        <Badge color={mode === "disabled" ? "zinc" : "green"}>
          {modes.find((option) => option.mode === mode)?.label}
          {mode !== "disabled" && " · активно"}
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

// State of the app's listener only; each page decides what it means for its app.
function ProxyStatus({ proxy: { proxy, error }, app }: { proxy: ReturnType<typeof useProxy>; app: keyof Proxy }) {
  if (error !== undefined) return <Badge color="amber" title={error}>Состояние недоступно</Badge>;
  if (proxy === undefined) return <Badge color="zinc">Загрузка…</Badge>;

  const running = proxy[app];
  return <Badge color={running ? "green" : "zinc"}>{running ? "Работает" : "Остановлен"}</Badge>;
}

function BackupButton({ agent, disabled }: { agent: BackupAgent; disabled: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button outline disabled={disabled} onClick={() => setOpen(true)}>Резервные копии</Button>
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
  const hub: Hub = { ...useHubTokens(account.account?.active_hub_id), signedIn: account.signedIn };
  const claudeDesktopToken = useTokenMenu("claude_desktop", agents.claude_desktop.status === "found" ? agents.claude_desktop.data.token : null, hub);
  const claudeInstall = useClaudeDesktopInstall();

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

  const signIn = !account.signedIn && (
    <Fieldset className="mt-10">
      <Legend>CoreInfra AI Hub</Legend>
      <Text>Войдите в CoreInfra AI Hub, чтобы подключить агента. Вход в аккаунт не изменяет настройки агентов.</Text>
      <FieldGroup>
        <div>
          <Button outline disabled={!account.account || account.submitting} onClick={account.change}>
            {account.account?.pending ? "Ожидание входа…" : "Войти через CoreInfra Hub"}
          </Button>
          {/* The sidebar footer shows the error on wide windows; narrow ones hide the sidebar. */}
          {account.error && <div className="mt-3 lg:hidden"><ErrorText>{account.error}</ErrorText></div>}
        </div>
      </FieldGroup>
    </Fieldset>
  );

  let content: ReactNode;
  switch (page) {
    // Keep a prepared plan alive while the settings page is selected.
    case null:
      content = null;
      break;
    case "codex": {
      const codex = agents.codex;
      const chatGptProxyStopped = proxy.proxy?.chatgpt === false && codex.status === "found"
        && codex.data.desktop.status === "found" && codex.data.routing_mode !== "disabled";
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
                <ProxyStatus proxy={proxy} app="chatgpt" />
                {chatGptProxyStopped && <ErrorText>Прокси для ChatGPT не запущен, нажмите «Применить».</ErrorText>}
              </DescriptionDetails>
              {codex.status === "found" && (
                <RoutingRows modes={routingModes("ChatGPT")} mode={codex.data.routing_mode} agentToken={codex.data.token} hub={hub} />
              )}
            </DescriptionList>
          )}
          {codex.status === "found" && signIn}
          {codex.status === "found" && (
            <RoutingFieldset agent="codex" modes={routingModes("ChatGPT")} mode={codex.data.routing_mode}
              toChange={(mode) => ({ CodexSetRouting: mode })} agentToken={codex.data.token} hub={hub}
              note="Настройка общая для ChatGPT и Codex." disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
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
              <RoutingRows modes={routingModes("Claude")} mode={claude.data.routing_mode} agentToken={claude.data.token} hub={hub} />
            )}
          </DescriptionList>
          {claude.status === "found" && signIn}
          {claude.status === "found" && (
            <RoutingFieldset agent="claude" modes={routingModes("Claude")} mode={claude.data.routing_mode}
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
          <PageHeading title="Claude Desktop" />
          <DescriptionList className="mt-8 sm:mt-10">
            <DescriptionTerm>Версия</DescriptionTerm>
            <DescriptionDetails><Version detection={desktop} /></DescriptionDetails>
            {desktop.status === "found" && (
              <>
                <DescriptionTerm>Прокси</DescriptionTerm>
                <DescriptionDetails><ProxyStatus proxy={proxy} app="claude_desktop" /></DescriptionDetails>
                <RoutingRows modes={routingModes("Claude")} mode={desktop.data.routing_mode} agentToken={desktop.data.token} hub={hub} />
              </>
            )}
          </DescriptionList>
          {signIn}
          {desktop.status === "found" && (
            <RoutingFieldset agent="claude_desktop" token={claudeDesktopToken} modes={routingModes("Claude")} mode={desktop.data.routing_mode}
              toChange={(mode) => ({ ClaudeDesktopSetRouting: mode })} agentToken={desktop.data.token} hub={hub}
              note="Для входа в аккаунт включите VPN и примените режим «Без CoreInfra» в Wizard."
              disabled={isChangeInProgress} error={pageError} onPrepare={prepare} />
          )}
          <Fieldset className="mt-10">
            <Legend>{desktop.status === "found" ? "Обновление приложения" : "Установка приложения"}</Legend>
            <FieldGroup>
              {desktop.status !== "found" && <TokenField token={claudeDesktopToken} hub={hub} disabled={isChangeInProgress} />}
              <ClaudeDesktopInstall observation={claudeInstall} token={claudeDesktopToken.selected} tokenReady={claudeDesktopToken.ready}
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
              <RoutingRows modes={hubModes} mode={detection.data.routing_mode} agentToken={detection.data.token} hub={hub} />
            )}
          </DescriptionList>
          {detection.status === "found" && signIn}
          {detection.status === "found" && (
            <RoutingFieldset agent={page} modes={hubModes} mode={detection.data.routing_mode}
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
      {preparedChange && <PreparedPlanDialog prepared={preparedChange} onClose={() => setPreparedChange(undefined)} />}
    </>
  );
}
