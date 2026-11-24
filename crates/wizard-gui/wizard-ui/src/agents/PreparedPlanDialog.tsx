import { type ReactNode, useEffect, useRef, useState } from "react";
import { Disclosure, DisclosureButton, DisclosurePanel } from "@headlessui/react";
import { ChevronRightIcon } from "@heroicons/react/16/solid";
import { invoke } from "@tauri-apps/api/core";
import {
  type ConnectionPlan,
  applyAgentPlan,
  applyMediaConnection,
  prepareAgentChange,
} from "./agents_state";
import { reportError } from "../logs/log";
import { ErrorText } from "../components/ErrorText";
import { Button } from "../components/catalyst/button";
import { Dialog, DialogActions, DialogBody } from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import { Code, Text } from "../components/catalyst/text";
import type { AgentChoice } from "../onboarding/agents";
import { agentPageNames } from "./navigation";

const visibleSteps = 2;
const plural = new Intl.PluralRules("ru");
function stepCount(count: number) {
  const form = plural.select(count);
  return count + " " + (form === "one" ? "шаг" : form === "few" ? "шага" : "шагов");
}

function PlanSteps({ plan }: { plan: ConnectionPlan }) {
  const steps: ReactNode[] = (plan.routing?.info ?? []).map(step => step.description);
  // Keep the choice from the form visible alongside the first routing action.
  if (plan.mediaEnabled !== undefined) {
    const server = <Code>coreinfra_media</Code>;
    const skill = <Code>coreinfra-video</Code>;
    if (plan.agent === "claude_desktop") {
      steps.unshift(plan.mediaEnabled
        ? <>Подключит MCP-сервер {server} в Claude Desktop.</>
        : <>Отключит MCP-сервер {server} в Claude Desktop.</>);
    } else {
      steps.unshift(plan.mediaEnabled
        ? <>Установит скилл {skill} и подключит MCP-сервер {server}.</>
        : <>Отключит MCP-сервер {server} и удалит скилл {skill}, установленный Wizard.</>);
    }
  }
  const rows = steps.map((step, index) => <li key={index}><Text>{step}</Text></li>);
  return (
    <>
      <ul className="agent-plan-steps">{rows.slice(0, visibleSteps)}</ul>
      {rows.length > visibleSteps && (
        <Disclosure>
          <DisclosureButton className="agent-plan-disclosure">
            {({ open }) => <>
              <ChevronRightIcon aria-hidden="true" />
              {open ? "Скрыть подробности" : "Ещё " + stepCount(rows.length - visibleSteps)}
            </>}
          </DisclosureButton>
          <DisclosurePanel>
            <ul className="agent-plan-steps">{rows.slice(visibleSteps)}</ul>
          </DisclosurePanel>
        </Disclosure>
      )}
    </>
  );
}

export function PreparedPlanDialog({
  prepared, launchChoice, onReprepared, onClose, onApplied,
  applyLabel = "Завершить настройку",
}: {
  prepared: ConnectionPlan;
  launchChoice: AgentChoice;
  onReprepared: (prepared: ConnectionPlan) => void;
  onClose: () => void;
  onApplied?: () => void;
  applyLabel?: "Применить" | "Завершить настройку";
}) {
  const { agent, routing, mediaEnabled } = prepared;
  const hasChanges = Boolean(routing?.info.length) || mediaEnabled !== undefined;
  const [operation, setOperation] = useState<"apply" | "prepare" | "launch">();
  const [routingApplied, setRoutingApplied] = useState(false);
  const [applied, setApplied] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
  const success = useRef<HTMLDivElement>(null);
  const busy = operation !== undefined;

  useEffect(() => {
    if (applied) success.current?.querySelector<HTMLElement>("h2")?.focus();
  }, [applied]);

  function finish() {
    onApplied?.();
    onClose();
  }

  async function reprepare() {
    if (inFlight.current || !routing || routingApplied) return;
    inFlight.current = true;
    setOperation("prepare");
    try {
      const token = routing.token === null ? null : { kind: "existing" as const, token: routing.token };
      const next = await prepareAgentChange({ agent, routing: { change: routing.change, token }, mediaEnabled });
      onReprepared(next);
      setError(undefined);
    } catch (cause) {
      reportError("Не удалось пересоздать план", cause);
      setError("Не удалось пересоздать план. " + String(cause));
    } finally {
      inFlight.current = false;
      setOperation(undefined);
    }
  }

  async function apply() {
    if (inFlight.current || !hasChanges || applied) return;
    inFlight.current = true;
    setOperation("apply");
    setError(undefined);
    let applyingMedia = false;
    try {
      if (routing?.info.length && !routingApplied) {
        await applyAgentPlan(routing.plan);
        setRoutingApplied(true);
      }
      if (mediaEnabled !== undefined) {
        // A routing plan may snapshot the same agent files. Finish it before changing media.
        applyingMedia = true;
        await applyMediaConnection(agent, mediaEnabled);
      }
      setApplied(true);
    } catch (cause) {
      const message = applyingMedia ? "Не удалось применить настройку медиа." : "Не удалось применить изменения. Часть изменений могла быть применена.";
      reportError(message, cause);
      setError(message + " " + String(cause));
    } finally {
      inFlight.current = false;
      setOperation(undefined);
    }
  }

  async function launch() {
    if (inFlight.current) return;
    inFlight.current = true;
    setOperation("launch");
    setError(undefined);
    try {
      await invoke("launch_agent", { agent: launchChoice.id });
      finish();
    } catch (cause) {
      const message = "Не удалось запустить " + launchChoice.name + ".";
      reportError(message, cause);
      setError(message + " " + String(cause));
    } finally {
      inFlight.current = false;
      setOperation(undefined);
    }
  }

  function close() {
    if (inFlight.current) return;
    if (applied || !hasChanges) finish();
    else onClose();
  }

  return (
    <Dialog open onClose={close}>
      {applied ? (
        <div ref={success}>
          <DialogHeader
            title="Настройки применены"
            description={launchChoice.desktop
              ? routing?.restarts_desktop
                ? "Запустите " + launchChoice.name + ", чтобы продолжить с новыми настройками."
                : "Можно продолжить работу в " + launchChoice.name + "."
              : launchChoice.name + " откроется в терминале."}
          />
        </div>
      ) : (
        <>
          <DialogHeader
            title={`План изменений: ${agentPageNames[agent]}`}
            onClose={close}
            disabled={busy}
            description={routing?.info.length
              ? routing.restarts_desktop
                ? "Завершите текущую работу. Wizard закроет приложение, если оно запущено, и применит настройки."
                : "Завершите текущую работу и закройте агента перед применением."
              : undefined}
          />
          <DialogBody>
            {hasChanges ? <PlanSteps plan={prepared} /> : <Text>Изменения не требуются.</Text>}
          </DialogBody>
        </>
      )}
      {error && <div className="mt-4" role="alert"><ErrorText>{error}</ErrorText></div>}
      <DialogActions>
        <Button plain disabled={busy} onClick={close}>
          {applied ? "Готово" : hasChanges ? "Отмена" : "Закрыть"}
        </Button>
        {applied ? (
          <Button disabled={busy} onClick={() => void launch()}>
            {operation === "launch" ? "Запускаем…" : "Запустить агента"}
          </Button>
        ) : (
          <>
            {error && routing && !routingApplied && (
              <Button outline disabled={busy} onClick={() => void reprepare()}>
                {operation === "prepare" ? "Пересоздание…" : "Пересоздать"}
              </Button>
            )}
            {hasChanges && <Button disabled={busy} onClick={() => void apply()}>
              {operation === "apply" ? "Сохраняем…" : applyLabel}
            </Button>}
          </>
        )}
      </DialogActions>
    </Dialog>
  );
}
