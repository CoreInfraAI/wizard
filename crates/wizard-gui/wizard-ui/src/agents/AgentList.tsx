import { type ReactNode, useEffect, useState } from "react";
import { type AgentChange, type AgentDetection, type AgentStates, type BackupAgent, type PreparedAgentChange, type ProxyMode, prepareAgentChange } from "./agents_state";
import { PreparedPlanDialog } from "./PreparedPlanDialog";
import { BackupDialog } from "./BackupDialog";
import { useSettings } from "../settings/settings";
import { useProxy } from "./proxy";
import { reportError } from "../logs/log";
import { Button } from "../components/catalyst/button";
import { Badge } from "../components/catalyst/badge";
import { Heading } from "../components/catalyst/heading";
import { Label } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

const proxyOptions = [
  { mode: "disabled", label: "Без CoreInfra" },
  { mode: "proxy_hub", label: "CoreInfra Hub" },
  { mode: "proxy_api", label: "CoreInfra API" },
] as const;

function HubProxy({ agent, installed, disabled, onPrepare }: {
  agent: "Pi" | "OpenCode";
  installed: boolean;
  disabled: boolean;
  onPrepare: (change: AgentChange) => void;
}) {
  const { settings, error: settingsError } = useSettings();
  const hasToken = (settings?.coreinfra_api_key ?? "") !== "" && settingsError === undefined;
  const [selected, setSelected] = useState(installed);

  function prepare() {
    switch (agent) {
      case "Pi": onPrepare({ SetPiHub: selected }); break;
      case "OpenCode": onPrepare({ SetOpenCodeHub: selected }); break;
    }
  }

  return (
    <>
      <div>
        <Badge color={installed ? "green" : "zinc"}>
          {installed ? "CoreInfra Hub · активно" : "Без CoreInfra"}
        </Badge>
      </div>
      <RadioGroup
        value={selected ? "hub" : "none"}
        disabled={disabled}
        onChange={(value) => setSelected(value === "hub")}
      >
        <RadioField>
          <Radio value="none" />
          <Label>Без CoreInfra</Label>
        </RadioField>
        <RadioField>
          <Radio value="hub" />
          <Label>CoreInfra Hub</Label>
        </RadioField>
      </RadioGroup>
      <Button outline type="button" disabled={disabled || (selected && !hasToken)} onClick={prepare}>
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

function BackupButton({ agent, disabled }: { agent: BackupAgent; disabled: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button plain disabled={disabled} onClick={() => setOpen(true)}>Резервные копии…</Button>
      {open && <BackupDialog agent={agent} onClose={() => setOpen(false)} />}
    </>
  );
}

function ProxyStatus() {
  const { proxy, error } = useProxy();

  if (error !== undefined) return <Badge color="amber" title={error}>Прокси: состояние недоступно</Badge>;
  if (proxy === undefined) return <Badge color="zinc">Прокси: загрузка…</Badge>;

  return (
    <Badge color={proxy.running ? "green" : "zinc"}>
      {proxy.running ? "Прокси работает" : "Прокси остановлен"}
    </Badge>
  );
}

function AgentProxy({ agent, mode, disabled, onPrepare }: {
  agent: "codex" | "claude" | "claude_desktop";
  mode: ProxyMode;
  disabled: boolean;
  onPrepare: (change: AgentChange) => void;
}) {
  const { settings, error: settingsError } = useSettings();
  const hasToken = (settings?.coreinfra_api_key ?? "") !== "" && settingsError === undefined;
  const [draft, setDraft] = useState<ProxyMode>();
  const selected = draft ?? mode;

  function prepare() {
    switch (agent) {
      case "codex": onPrepare({ CodexSetProxy: selected }); break;
      case "claude": onPrepare({ ClaudeSetProxy: selected }); break;
      case "claude_desktop": onPrepare({ ClaudeDesktopSetProxy: selected }); break;
    }
  }

  useEffect(() => {
    if (draft !== undefined && draft === mode) setDraft(undefined);
  }, [draft, mode]);

  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Badge color={mode === "disabled" ? "zinc" : "green"}>
          {proxyOptions.find((option) => option.mode === mode)?.label}
          {mode !== "disabled" && " · активно"}
        </Badge>
        {agent === "claude_desktop" && <ProxyStatus />}
      </div>
      <RadioGroup
        value={selected}
        disabled={disabled}
        onChange={(value) => setDraft(value as ProxyMode)}
      >
        {proxyOptions.map((option) => (
          <RadioField key={option.mode}>
            <Radio value={option.mode} />
            <Label>
              {option.label}{option.mode === "proxy_api"
                ? ` — через вашу подписку ${agent === "codex" ? "ChatGPT" : "Claude"}`
                : ""}
            </Label>
          </RadioField>
        ))}
      </RadioGroup>
      <Button outline type="button" disabled={disabled || (selected !== "disabled" && !hasToken)} onClick={prepare}>
        Применить
      </Button>
    </>
  );
}

export function AgentList({ agents }: { agents: AgentStates }) {
  const [preparedChange, setPreparedChange] = useState<PreparedAgentChange>();
  const [isPreparingPlan, setIsPreparingPlan] = useState(false);
  const [error, setError] = useState<string>();
  const isChangeInProgress = isPreparingPlan || preparedChange !== undefined;

  async function prepare(change: AgentChange) {
    if (isChangeInProgress) return;

    setIsPreparingPlan(true);
    setError(undefined);

    try {
      setPreparedChange(await prepareAgentChange(change));
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
      <Installation name="Codex" detection={agents.codex} actions={<BackupButton agent="codex" disabled={isChangeInProgress} />}>
        {agents.codex.status === "found" && (
          <AgentProxy agent="codex" mode={agents.codex.data.proxy_mode} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      {/*<Installation name="ChatGPT" detection={agents.codex_desktop} />*/}
      <Installation name="Claude Code" detection={agents.claude} actions={<BackupButton agent="claude" disabled={isChangeInProgress} />}>
        {agents.claude.status === "found" && (
          <AgentProxy agent="claude" mode={agents.claude.data.proxy_mode} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      <Installation name="Claude Desktop" detection={agents.claude_desktop}>
        {agents.claude_desktop.status === "found" && (
          <AgentProxy agent="claude_desktop" mode={agents.claude_desktop.data.proxy_mode} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      <Installation name="OpenCode" detection={agents.opencode}>
        {agents.opencode.status === "found" && (
          <HubProxy agent="OpenCode" installed={agents.opencode.data.proxy_installed} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      <Installation name="Pi" detection={agents.pi}>
        {agents.pi.status === "found" && (
          <HubProxy agent="Pi" installed={agents.pi.data.proxy_installed} disabled={isChangeInProgress} onPrepare={prepare} />
        )}
      </Installation>
      {error && <div className="col-span-full"><ErrorText>{error}</ErrorText></div>}
      {preparedChange && <PreparedPlanDialog prepared={preparedChange} onClose={() => setPreparedChange(undefined)} />}
    </>
  );
}
