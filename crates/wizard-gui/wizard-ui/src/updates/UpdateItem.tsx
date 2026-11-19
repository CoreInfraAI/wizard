import { ArrowDownTrayIcon } from "@heroicons/react/20/solid";
import { SidebarItem, SidebarLabel } from "../components/catalyst/sidebar";
import { ErrorText } from "../components/ErrorText";
import { AppLogo } from "../components/AppLogo";
import { TransferCard } from "../components/ui/TransferCard";
import { useSidebarAction } from "../components/catalyst/sidebar-layout";
import { megabytes } from "../agents/desktop_install_state";
import { type UpdateObservation, updateProgress } from "./update";

// Sidebar item for an available Wizard update or a failed update check.
export function UpdateItem({ state, onOpen }: {
  state: UpdateObservation;
  onOpen: () => void;
}) {
  const sidebarAction = useSidebarAction();
  const current = state.state;
  const error = state.error ?? (current?.status === "failed" ? current.message : undefined);

  if (current && (current.status === "downloading" || current.status === "ready_to_install" || current.status === "installing" || (current.status === "failed" && current.version))) {
    return <TransferCard variant="compact" brand="wizard" name="Wizard" icon={<AppLogo markOnly />}
      status={current.status === "downloading"
        ? `${megabytes(current.received)} / ${current.total ? megabytes(current.total) : "…"} МБ`
        : current.status === "ready_to_install" ? "Готово к установке"
        : current.status === "installing" ? "Устанавливается…" : "Не удалось обновить"}
      downloading={current.status === "downloading"} progress={updateProgress(current)}
      filled={current.status === "ready_to_install" || current.status === "installing"}
      tone={current.status === "failed" ? "error" : "default"}
      onOpen={() => sidebarAction(onOpen)} />;
  }

  return (
    <>
      {state.error === undefined && current?.status === "available" && (
        <SidebarItem onClick={() => sidebarAction(onOpen)}>
          <ArrowDownTrayIcon />
          <SidebarLabel>Обновить до v{current.version}</SidebarLabel>
        </SidebarItem>
      )}
      {error !== undefined && <div className="px-2"><ErrorText>{error}</ErrorText></div>}
    </>
  );
}
