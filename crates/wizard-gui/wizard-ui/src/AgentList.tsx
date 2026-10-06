import { type ReactNode, useState } from "react";
import { type AgentDetection, type AgentStates, type ProxyMode, sendAgentEvent } from "./agents_state";
import { reportError } from "./log";
import { Button } from "./components/catalyst/button";
import { Badge } from "./components/catalyst/badge";
import { Heading } from "./components/catalyst/heading";
import { Label } from "./components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "./components/catalyst/radio";
import { Text } from "./components/catalyst/text";
import { ErrorText } from "./components/ErrorText";

const proxyOptions = [
  { mode: "disabled", label: "Без CoreInfra" },
  { mode: "proxy_hub", label: "CoreInfra Hub" },
  { mode: "proxy_api", label: "CoreInfra API" },
] as const;

function AgentProxy({ agent, mode, token }: {
  agent: "codex" | "claude";
  mode: ProxyMode;
  token: string;
}) {
  const [selected, setSelected] = useState(mode);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();

  async function send() {
    setPending(true);
    setError(undefined);
    try {
      await sendAgentEvent(agent === "codex"
        ? { CodexSetProxy: selected }
        : { ClaudeSetProxy: selected });
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
        onChange={(value) => {
          setSelected(value as ProxyMode);
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
      <Button outline type="button" disabled={pending || (selected !== "disabled" && token === "")} onClick={() => void send()}>
        Применить
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
    </>
  );
}

function HubProxy({ agent, installed, token }: {
  agent: "Pi" | "OpenCode";
  installed: boolean;
  token: string;
}) {
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
      <Button outline type="button" disabled={pending || (selected && token === "")} onClick={() => void send()}>
        Применить
      </Button>
      {error && <ErrorText>{error}</ErrorText>}
    </>
  );
}

function Installation({ name, detection, children }: {
  name: string;
  detection: AgentDetection<{ path: string; version: string | null }>;
  children?: ReactNode;
}) {
  return (
    <section className="flex min-w-0 flex-col gap-3 rounded-lg border border-zinc-200 bg-white p-5 sm:p-6 dark:border-zinc-800 dark:bg-zinc-900">
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <Heading level={2}>{name}</Heading>
        {detection.status === "found" && (
          <span className="text-sm text-zinc-500 dark:text-zinc-400">
            {detection.data.version ? `v${detection.data.version}` : "Версия неизвестна"}
          </span>
        )}
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
      {detection.status === "not_found" && <Text>Not found</Text>}
      {detection.status === "error" && (
        <ErrorText>Не удалось обнаружить {name}: {detection.data}</ErrorText>
      )}
    </section>
  );
}

export function AgentList({ agents, token }: { agents: AgentStates; token: string }) {
  return (
    <>
      <Installation name="Codex" detection={agents.codex}>
        {agents.codex.status === "found" && (
          <AgentProxy agent="codex" mode={agents.codex.data.proxy_mode} token={token} />
        )}
      </Installation>
      <Installation name="ChatGPT" detection={agents.chatgpt} />
      <Installation name="Claude Code" detection={agents.claude}>
        {agents.claude.status === "found" && (
          <AgentProxy agent="claude" mode={agents.claude.data.proxy_mode} token={token} />
        )}
      </Installation>
      <Installation name="Claude Desktop" detection={agents.claude_desktop} />
      <Installation name="OpenCode" detection={agents.opencode}>
        {agents.opencode.status === "found" && (
          <HubProxy agent="OpenCode" installed={agents.opencode.data.proxy_installed} token={token} />
        )}
      </Installation>
      <Installation name="Pi" detection={agents.pi}>
        {agents.pi.status === "found" && (
          <HubProxy agent="Pi" installed={agents.pi.data.proxy_installed} token={token} />
        )}
      </Installation>
    </>
  );
}
