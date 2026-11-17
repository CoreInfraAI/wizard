import { useEffect, useState } from "react";
import { useDesktopInstall, type DesktopApp } from "../agents/DesktopInstall";
import type { useAgentState } from "../agents/agents_state";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Label } from "../components/catalyst/fieldset";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { AgentIcon, choices, isFound, type ChoiceId } from "./agents";
import { InstallCard } from "./InstallCard";

export type InstallView = "choose" | "progress" | "confirm";

export function AgentSelection({ mode, agents, signedIn, selected, onSelect, onContinue, onClose, onInstallViewChange }: {
  mode: "install" | "select";
  agents: ReturnType<typeof useAgentState>;
  signedIn: boolean;
  selected?: ChoiceId;
  onSelect: (id: ChoiceId) => void;
  onContinue: (id: ChoiceId) => void;
  onClose: () => void;
  onInstallViewChange: (view: InstallView) => void;
}) {
  const claude = useDesktopInstall("claude");
  const chatgpt = useDesktopInstall("chatgpt");
  const [focusedDesktop, setFocusedDesktop] = useState<DesktopApp>();
  // Cached downloads and old failures leave both choices visible on reopening.
  const resumedDesktop = [claude, chatgpt].find(({ inProgress }) => inProgress)?.desktop;
  const activeDesktop = focusedDesktop ?? resumedDesktop;
  const activeInstaller = activeDesktop === "claude" ? claude : activeDesktop === "chatgpt" ? chatgpt : undefined;
  const confirming = activeInstaller?.state?.status === "ready_to_install";
  const installView = activeDesktop ? confirming ? "confirm" : "progress" : "choose";
  useEffect(() => {
    if (mode === "install") onInstallViewChange(installView);
  }, [mode, installView, onInstallViewChange]);
  // Keep a resumed operation expanded through its result, until the dialog closes.
  useEffect(() => {
    if (resumedDesktop) setFocusedDesktop((current) => current ?? resumedDesktop);
  }, [resumedDesktop]);
  const claudeState = agents.status === "ready" ? agents.data.claude_desktop : undefined;
  const codexState = agents.status === "ready" ? agents.data.codex : undefined;
  const installed = agents.status === "ready" ? choices.filter((choice) => isFound(choice, agents.data)) : [];
  const selectionReady = installed.some((choice) => choice.id === selected);
  const detectionErrors = agents.status === "ready" && Object.values(agents.data).some((value) => typeof value === "object" && value.status === "error");

  return <div>
    {!(mode === "install" && confirming) && <Heading level={2} className="font-semibold!" tabIndex={-1}>{mode === "install" ? activeDesktop ? "Установка агента" : "Установите агентов" : "Выберите агента"}</Heading>}
    {(mode === "select" || agents.status !== "ready") && <Text className="mt-2">{agents.status === "loading"
      ? "Ищем установленных агентов…" : agents.status === "error"
        ? "Не удалось проверить приложения. Проверка повторится автоматически." : installed.length
          ? "Найденные на этом компьютере" : "Установленные агенты не найдены. Вернитесь в обзор, чтобы установить приложение."}</Text>}
    {agents.status === "error" && <div className="mt-4" role="alert"><ErrorText>{agents.message}</ErrorText></div>}
    {detectionErrors && <Text className="mt-3" role="status">Не все приложения удалось проверить. Список обновится автоматически.</Text>}
    {agents.status === "ready" && <div>
      {mode === "select" && installed.length > 0 && <RadioGroup value={selected ?? ""} onChange={(value) => onSelect(value as ChoiceId)} aria-label="Выберите агента" className="onboarding-agent-list mt-5 grid grid-cols-1 gap-2 sm:grid-cols-2">
        {installed.map((choice) => <RadioField key={choice.id} className="onboarding-agent-row items-center" data-selected={selected === choice.id}>
          <Radio value={choice.id} className="mt-0!" />
          <Label className="flex cursor-pointer items-center gap-3"><AgentIcon choice={choice} className="size-5!" />{choice.name}</Label>
        </RadioField>)}
      </RadioGroup>}
      {mode === "install" && <section aria-label="Установить агента" className="onboarding-install-grid" data-focused={Boolean(activeDesktop)} data-confirming={confirming}>
        {choices.filter((choice) => choice.desktop).map((choice) => {
          const installer = choice.desktop === "claude" ? claude : chatgpt;
          const found = installed.some((item) => item.id === choice.id);
          const version = choice.desktop === "claude"
            ? claudeState?.status === "found" ? claudeState.data.version : null
            : codexState?.status === "found" && codexState.data.desktop.status === "found" ? codexState.data.desktop.data.version : null;
          return <InstallCard key={choice.id} choice={choice} observation={installer} signedIn={signedIn}
            found={found} version={version} active={activeDesktop === choice.desktop}
            hidden={Boolean(activeDesktop && activeDesktop !== choice.desktop)}
            onStart={() => setFocusedDesktop(choice.desktop)} onDone={onClose} />;
        })}
      </section>}
      {mode === "install" && !signedIn && <Text className="mt-3">Войдите в CoreInfra Hub, чтобы скачать приложение.</Text>}
    </div>}
    {mode === "select" && <div className="onboarding-footer flex justify-end"><Button disabled={!selectionReady} onClick={() => { if (selected && selectionReady) onContinue(selected); }}>Настроить агента</Button></div>}
  </div>;
}
