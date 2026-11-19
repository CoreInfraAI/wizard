import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  useDesktopInstall,
  type DesktopApp,
  type Snapshot,
} from "../agents/desktop_install_state";
import type { useAgentState } from "../agents/agents_state";
import { Dialog } from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import { ViewTransition } from "../components/ui/Layout";
import { StateNotice } from "../components/ui/StateNotice";
import { Text } from "../components/catalyst/text";
import { choices, isFound } from "./agents";
import { InstallCard } from "./InstallCard";
import {
  InstallationsContext,
  type Installations,
} from "./installations_state";
import "./SetupDialog.css";

/** The provider outlives pages and dialogs. Only explicit cancellation stops a transfer. */
export function InstallationsProvider({
  children,
  agents,
  signedIn,
}: {
  children: ReactNode;
  agents: ReturnType<typeof useAgentState>;
  signedIn: boolean;
}) {
  const claude = useDesktopInstall("claude");
  const chatgpt = useDesktopInstall("chatgpt");
  const observations = { claude, chatgpt };
  // Navigation belongs to this opening of the dialog, never to the background job.
  const [navigation, setNavigation] = useState<{ desktop: DesktopApp | null; fromPicker: boolean }>();
  const view = navigation?.desktop;
  const title = useRef<HTMLDivElement>(null);
  useEffect(() => {
    title.current
      ?.querySelector<HTMLElement>("h2")
      ?.focus({ preventScroll: true });
  }, [view]);
  const selectedStatus = view ? observations[view].state?.status : undefined;
  useEffect(() => {
    // A completed/cancelled operation may remove the focused action button.
    if (document.activeElement === document.body)
      title.current
        ?.querySelector<HTMLElement>("h2")
        ?.focus({ preventScroll: true });
  }, [selectedStatus]);
  const [requesting, setRequesting] = useState<Installations["requesting"]>({});
  const [errors, setErrors] = useState<Installations["errors"]>({});
  const [acknowledged, setAcknowledged] = useState<
    Partial<Record<DesktopApp, number>>
  >({});
  const inFlight = useRef(new Set<DesktopApp>());

  async function run(desktop: DesktopApp, action: () => Promise<unknown>) {
    if (inFlight.current.has(desktop)) return;
    inFlight.current.add(desktop);
    setRequesting((current) => ({ ...current, [desktop]: true }));
    setErrors((current) => ({ ...current, [desktop]: undefined }));
    try {
      await action();
    } catch (cause) {
      setErrors((current) => ({
        ...current,
        [desktop]:
          typeof cause === "string"
            ? cause
            : "Не удалось выполнить действие. Попробуйте ещё раз.",
      }));
    } finally {
      inFlight.current.delete(desktop);
      setRequesting((current) => ({ ...current, [desktop]: false }));
    }
  }

  const desktopChoices = choices.filter((choice) => choice.desktop);
  const jobs = desktopChoices
    .filter((choice) => {
      const state = observations[choice.desktop!].state;
      return (
        state?.status === "downloading" ||
        state?.status === "ready_to_install" ||
        state?.status === "installing" ||
        (state?.status === "installed" &&
          acknowledged[choice.desktop!] !== state.revision) ||
        (state?.status === "failed" && state.operation !== "check")
      );
    })
    .map((choice) => choice.desktop!);

  const controls: Installations = {
    observations,
    requesting,
    errors,
    jobs,
    open: (desktop) => setNavigation({ desktop: desktop ?? null, fromPicker: false }),
    download: (desktop) =>
      run(desktop, async () => {
        if (!signedIn) return;
        let state = observations[desktop].state;
        if (!state || observations[desktop].busy) return;
        if (
          state.status !== "available" &&
          !(state.status === "failed" && state.operation === "download")
        ) {
          await invoke("check_desktop_release", { desktop });
          state = await invoke<Snapshot>("get_desktop_install_state", {
            desktop,
            lastRevision: null,
          });
          while (state.status === "checking") {
            state = await invoke<Snapshot>("get_desktop_install_state", {
              desktop,
              lastRevision: state.revision,
            });
          }
        }
        // Checking may restore a complete archive, or find the installed version current.
        if (
          state.status === "available" ||
          (state.status === "failed" && state.operation === "download")
        ) {
          await invoke("download_desktop_release", { desktop });
        }
      }),
    cancel: (desktop) =>
      run(desktop, () => invoke("cancel_desktop_download", { desktop })),
    install: (desktop, confirmed, close) =>
      run(desktop, () =>
        invoke("install_desktop_release", {
          desktop,
          confirmed,
          close,
          openAfterInstall: false,
        }),
      ),
  };

  return (
    <InstallationsContext.Provider value={controls}>
      {children}
      {view !== undefined && (
        <Dialog
          open
          onClose={() => setNavigation(undefined)}
          size={view ? "md" : "2xl"}
          className={`setup-dialog relative${view ? " setup-dialog-compact" : ""}`}
        >
          <div ref={title}>
            <DialogHeader
              title={view ? "Установка агента" : "Установите агентов"}
              closeLabel="Свернуть установку"
              onClose={() => setNavigation(undefined)}
            />
          </div>
          <ViewTransition view={view ?? "picker"}>
            {agents.status === "ready" ? (
              <section
                aria-label="Установить агента"
                className="onboarding-install-grid"
                data-focused={Boolean(view)}
              >
                {desktopChoices
                  .filter((choice) => !view || choice.desktop === view)
                  .map((choice) => {
                    const desktop = choice.desktop!;
                    const detection =
                      desktop === "claude"
                        ? agents.data.claude_desktop
                        : agents.data.codex.status === "found"
                          ? agents.data.codex.data.desktop
                          : agents.data.codex;
                    return (
                      <InstallCard
                        key={desktop}
                        choice={choice}
                        observation={observations[desktop]}
                        signedIn={signedIn}
                        found={isFound(choice, agents.data)}
                        version={
                          detection.status === "found"
                            ? detection.data.version
                            : null
                        }
                        focused={view === desktop}
                        onSelect={() => setNavigation({ desktop, fromPicker: true })}
                        onBack={navigation?.fromPicker ? () => setNavigation({ desktop: null, fromPicker: false }) : undefined}
                        onDone={() => {
                          setAcknowledged((current) => ({
                            ...current,
                            [desktop]: observations[desktop].state?.revision,
                          }));
                          setNavigation(undefined);
                        }}
                      />
                    );
                  })}
              </section>
            ) : (
              <div className="mt-6">
                <StateNotice
                  tone={agents.status === "loading" ? "loading" : "error"}
                >
                  {agents.status === "loading"
                    ? "Ищем установленных агентов…"
                    : "Не удалось проверить приложения. Проверка повторится автоматически."}
                </StateNotice>
              </div>
            )}
            {!signedIn && (
              <Text className="mt-4">
                Войдите в CoreInfra Hub, чтобы скачать приложение.
              </Text>
            )}
          </ViewTransition>
        </Dialog>
      )}
    </InstallationsContext.Provider>
  );
}
