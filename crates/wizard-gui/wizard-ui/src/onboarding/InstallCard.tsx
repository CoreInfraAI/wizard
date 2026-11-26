import { useEffect, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Checkbox, CheckboxField } from "../components/catalyst/checkbox";
import { Label } from "../components/catalyst/fieldset";
import { Text } from "../components/catalyst/text";
import { TransferCard } from "../components/ui/TransferCard";
import { ActionBar } from "../components/ui/Layout";
import { ErrorText } from "../components/ErrorText";
import { CancelDownloadButton } from "../agents/DesktopInstall";
import {
  megabytes,
  useRunningApps,
  type useDesktopInstall,
} from "../agents/desktop_install_state";
import { AgentIcon, type AgentChoice } from "./agents";
import { downloadProgress, useInstallations } from "./installations_state";

/** Selection and expanded installation render the same live backend snapshot. */
export function InstallCard({
  choice,
  observation,
  signedIn,
  found,
  version,
  focused,
  onSelect,
  onBack,
  onDone,
}: {
  choice: AgentChoice;
  observation: ReturnType<typeof useDesktopInstall>;
  signedIn: boolean;
  found: boolean;
  version: string | null;
  focused: boolean;
  onSelect: () => void;
  onBack?: () => void;
  onDone: () => void;
}) {
  const { desktop, state } = observation;
  const controls = useInstallations();
  const requesting = controls.requesting[desktop];
  const [confirmed, setConfirmed] = useState(false);
  const retryInstall =
    state?.status === "failed" && state.operation === "install";
  const downloaded = state?.status === "ready_to_install";
  const installable = downloaded || retryInstall;
  const confirmation = installable ? state.confirmation : null;
  const releaseVersion = downloaded ? state.version : null;
  useEffect(() => setConfirmed(false), [confirmation, releaseVersion, focused]);
  const runningApps = useRunningApps(desktop, focused && installable);
  const { running } = runningApps;
  const downloading = state?.status === "downloading";
  const installing = state?.status === "installing";
  const installed = state?.status === "installed";
  const available = state?.status === "available";
  const current = installed || state?.status === "up_to_date";
  const checking =
    state?.status === "checking" || (state?.status === "idle" && signedIn);
  const canDownload =
    available ||
    (state?.status === "failed" && state.operation !== "install") ||
    (state?.status === "idle" && !found && !signedIn);
  const progress = downloadProgress(state);
  const failedMessage =
    controls.errors[desktop] ??
    observation.error ??
    (state?.status === "failed" ? state.message : undefined);
  const active = observation.busy || downloaded || retryInstall;
  const networkBlocked = !signedIn || Boolean(observation.error);
  const busy = Boolean(requesting || observation.busy);
  const actionLabel =
    state?.status === "unsupported"
      ? "Недоступно"
      : downloaded
        ? "Установить"
        : retryInstall
          ? "Повторить установку"
          : available && found
            ? "Скачать обновление"
            : state?.status === "failed"
              ? "Повторить"
              : "Скачать";
  const description =
    desktop === "claude"
      ? "Приложение Anthropic с Cowork и Claude Code."
      : "Приложение OpenAI с ChatGPT Work и Codex.";
  const status = !focused && current ? "" : downloading
    ? `Скачивается${progress === undefined ? "…" : ` · ${progress}%`}`
    : installing
      ? "Устанавливается…"
      : downloaded
        ? `Версия ${state.version} — готова к установке`
        : checking
          ? "Проверяем версию…"
          : installed
            ? `Установлена версия ${state.version}`
            : state?.status === "up_to_date"
              ? "Установлена актуальная версия"
              : available
                ? `Доступна версия ${state.version}`
                : state?.status === "unsupported"
                  ? state.message
                  : state?.status === "failed" && state.operation === "check"
                    ? "Не удалось проверить версию"
                    : found
                      ? ""
                      : description;

  function start() {
    if (!focused) { onSelect(); return; }
    if (active) return;
    if (canDownload) void controls.download(desktop);
  }

  return (
    <div className="onboarding-install-entry">
      <TransferCard
        name={choice.name}
        icon={<AgentIcon choice={choice} />}
        brand={desktop}
        version={
          found &&
          !downloading &&
          !downloaded &&
          !installing &&
          (!installed || !focused) &&
          !retryInstall
            ? (installed ? state.version : version)
              ? `Версия ${installed ? state.version : version}`
              : "Версия не определена"
            : undefined
        }
        status={status}
        detail={
          downloading
            ? `${megabytes(state.received)} / ${state.total > 0 ? megabytes(state.total) : "…"} МБ`
            : undefined
        }
        notice={
          downloading && state.reconnecting
            ? "Восстанавливаем соединение…"
            : undefined
        }
        error={!focused ? failedMessage : undefined}
        progress={progress}
        downloading={downloading}
        filled={downloaded || installing || installed}
        tone={
          installed
            ? "success"
            : state?.status === "failed"
              ? "error"
              : "default"
        }
        onOpen={!focused && active ? onSelect : undefined}
        action={
          !focused && (active || canDownload) ? (
            <div className="ui-transfer-action-group">
              <Button
                outline
                disabled={
                  !state ||
                  state.status === "unsupported" ||
                  (!active && (busy || networkBlocked))
                }
                onClick={start}
              >
                {found ? "Обновить" : "Скачать"}
              </Button>
            </div>
          ) : undefined
        }
      />
      {focused && (
        <>
          {downloaded && (state.notes.length > 0 || !state.path) && (
            <div className="mt-4 space-y-2">
              {state.notes.map((note) => (
                <Text key={note}>{note}</Text>
              ))}
              {!state.path && (
                <Text>
                  Система запросит права администратора для установки{" "}
                  {choice.name}.
                </Text>
              )}
            </div>
          )}
          {installable && running.length > 0 && (
            <Text className="mt-4">
              Wizard закроет {running.join(", ")} и откроет снова. Сохраните
              текущую работу перед установкой.
            </Text>
          )}
          {installable && !runningApps.checked && (
            <Text className="mt-4">
              {runningApps.error ?? "Проверяем запущенные приложения…"}
            </Text>
          )}
          {installable && confirmation && (
            <CheckboxField className="mt-4" disabled={Boolean(requesting)}>
              <Checkbox checked={confirmed} onChange={setConfirmed} />
              <Label>{confirmation}</Label>
            </CheckboxField>
          )}
          {installing && (
            <Text className="mt-4">
              Не выходите из Wizard до завершения установки.
            </Text>
          )}
          {installed &&
            state.notices.map((notice) => (
              <Text key={notice} className="mt-4">
                {notice}
              </Text>
            ))}
          {failedMessage && (
            <div className="mt-4" role="alert">
              <ErrorText>{failedMessage}</ErrorText>
            </div>
          )}
          <ActionBar
            className="mt-6"
            leading={
              onBack && <Button plain onClick={onBack}>
                Назад
              </Button>
            }
          >
            {(downloaded ||
              (state?.status === "failed" &&
                state.operation === "download")) && (
              <CancelDownloadButton desktop={desktop} />
            )}
            {downloading ? (
              <CancelDownloadButton desktop={desktop} />
            ) : installing ? (
              <Button outline disabled>
                Устанавливаем…
              </Button>
            ) : current ? (
              <Button onClick={onDone}>Готово</Button>
            ) : (
              (installable || canDownload) && (
                <Button
                  disabled={
                    busy ||
                    Boolean(observation.error) ||
                    (installable
                      ? !runningApps.checked ||
                        (Boolean(confirmation) && !confirmed)
                      : networkBlocked)
                  }
                  onClick={() =>
                    installable
                      ? void controls.install(
                          desktop,
                          confirmed,
                          running.length > 0,
                        )
                      : start()
                  }
                >
                  {actionLabel}
                </Button>
              )
            )}
          </ActionBar>
        </>
      )}
    </div>
  );
}
