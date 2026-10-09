import { type ReactNode, useState } from "react";
import { type AgentDetection, type AgentStates, type BackupAgent, type ProxyMode, sendAgentEvent } from "./agents_state";
import { BackupDialog } from "./BackupDialog";
import { useSettings } from "../settings/settings";
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

function HubProxy({ agent, installed }: {
  agent: "Pi" | "OpenCode";
  installed: boolean;
}) {
  const { settings, error: settingsError } = useSettings();
  const hasToken = (settings?.coreinfra_api_key ?? "") !== "" && settingsError === undefined;
  const [selected, setSelected] = useState(installed);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send() {
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(agent === "Pi"
        ? { SetPiHub: selected }
        : { SetOpenCodeHub: selected });
    } catch (cause: unknown) {
      const message = `Не удалось применить настройки ${agent}`;
      reportError(message, cause);
      setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setPending(false);
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
        onChange={(value) => {
          setSelected(value === "hub");
          setError(undefined);
        }}
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
      <Button outline type="button" disabled={pending || (selected && !hasToken)} onClick={() => void send()}>
        Применить
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
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

function BackupButton({ agent }: { agent: BackupAgent }) {
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button plain onClick={() => setOpen(true)}>Резервные копии…</Button>
      {open && <BackupDialog agent={agent} onClose={() => setOpen(false)} />}
    </>
  );
}

function AgentProxy({ agent, mode }: {
  agent: BackupAgent;
  mode: ProxyMode;
}) {
  const { settings, error: settingsError } = useSettings();
  const hasToken = (settings?.coreinfra_api_key ?? "") !== "" && settingsError === undefined;
  const [pending, setPending] = useState(false);
  const [draft, setDraft] = useState<ProxyMode>();
  const [error, setError] = useState<string>();
  const selected = draft ?? mode;

  async function send() {
    if (pending) return;
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(agent === "codex"
        ? { CodexSetProxy: selected }
        : { ClaudeSetProxy: selected });
      setDraft(undefined);
    } catch (cause: unknown) {
      const message = `Не удалось применить настройки ${agent === "codex" ? "Codex" : "Claude Code"}`;
      reportError(message, cause);
      setError(`${message}: ${cause instanceof Error ? cause.message : String(cause)}`);
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <div>
        <Badge color={mode === "disabled" ? "zinc" : "green"}>
          {proxyOptions.find((option) => option.mode === mode)?.label}
          {mode !== "disabled" && " · активно"}
        </Badge>
      </div>
      <RadioGroup
        value={selected}
        disabled={pending}
        onChange={(value) => {
          setDraft(value as ProxyMode);
          setError(undefined);
        }}
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
      <Button outline type="button" disabled={pending || (selected !== "disabled" && !hasToken)} onClick={() => void send()}>
        Применить
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
    </>
  );
}

export function AgentList({ agents }: { agents: AgentStates }) {
  return (
    <>
      <Installation name="Codex" detection={agents.codex} actions={<BackupButton agent="codex" />}>
        {agents.codex.status === "found" && (
          <AgentProxy agent="codex" mode={agents.codex.data.proxy_mode} />
        )}
      </Installation>
      {/*<Installation name="ChatGPT" detection={agents.chatgpt} />*/}
      <Installation name="Claude Code" detection={agents.claude} actions={<BackupButton agent="claude" />}>
        {agents.claude.status === "found" && (
          <AgentProxy agent="claude" mode={agents.claude.data.proxy_mode} />
        )}
      </Installation>
      {/*<Installation name="Claude Desktop" detection={agents.claude_desktop} />*/}
      <Installation name="OpenCode" detection={agents.opencode}>
        {agents.opencode.status === "found" && (
          <HubProxy agent="OpenCode" installed={agents.opencode.data.proxy_installed} />
        )}
      </Installation>
      <Installation name="Pi" detection={agents.pi}>
        {agents.pi.status === "found" && (
          <HubProxy agent="Pi" installed={agents.pi.data.proxy_installed} />
        )}
      </Installation>
    </>
  );
}
