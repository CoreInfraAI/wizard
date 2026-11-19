import { megabytes } from "./agents/desktop_install_state";
import { AgentIcon, choices } from "./onboarding/agents";
import {
  downloadProgress,
  useInstallations,
} from "./onboarding/installations_state";
import { TransferCard } from "./components/ui/TransferCard";
import { useSidebarAction } from "./components/catalyst/sidebar-layout";
import "./SidebarDownloads.css";

/** Downloads are part of the sidebar, directly above the account divider. */
export function SidebarDownloads() {
  const { jobs, observations, open } = useInstallations();
  const runSidebarAction = useSidebarAction();
  if (!jobs.length) return null;
  return (
    <section className="sidebar-downloads" aria-label="Загрузки агентов">
      {jobs.map((desktop) => {
        const choice = choices.find((choice) => choice.desktop === desktop)!;
        const state = observations[desktop].state!;
        const downloading = state.status === "downloading";
        const installed = state.status === "installed";
        const ready = state.status === "ready_to_install";
        const failed = state.status === "failed";
        return (
          <TransferCard
            key={desktop}
            variant="compact"
            brand={desktop}
            name={choice.name}
            icon={<AgentIcon choice={choice} />}
            status={
              downloading
                ? `${megabytes(state.received)} / ${state.total > 0 ? megabytes(state.total) : "…"} МБ`
                : ready
                  ? "Готово к установке"
                  : installed
                    ? "Установлено"
                    : failed
                      ? state.operation === "install"
                        ? "Не удалось установить"
                        : "Не удалось скачать"
                      : "Устанавливается…"
            }
            notice={
              downloading && state.reconnecting ? "Переподключение…" : undefined
            }
            progress={downloadProgress(state)}
            downloading={downloading}
            filled={ready || installed || state.status === "installing"}
            tone={installed ? "success" : failed ? "error" : "default"}
            onOpen={() => runSidebarAction(() => open(desktop))}
          />
        );
      })}
    </section>
  );
}
