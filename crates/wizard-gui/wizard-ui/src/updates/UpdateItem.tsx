import { ArrowDownTrayIcon, ArrowPathIcon } from "@heroicons/react/20/solid";
import { SidebarItem, SidebarLabel } from "../components/catalyst/sidebar";
import { ErrorText } from "../components/ErrorText";
import { reportError } from "../logs/log";
import { type UpdateObservation, requestUpdate, retryUpdateCheck } from "./update";

// Sidebar item for an available Wizard update or a failed update check.
export function UpdateItem({ state }: {
  state: UpdateObservation;
}) {
  const current = state.state;
  const error = state.error ?? (current?.status === "failed" ? current.message : undefined);

  if (error === undefined && current?.status !== "available") return null;

  function request() {
    const action = current?.status === "failed" ? retryUpdateCheck : requestUpdate;
    void action().catch((cause: unknown) => {
      reportError("Не удалось запустить обновление", cause);
    });
  }

  return (
    <>
      {state.error === undefined && (
        <SidebarItem onClick={request}>
          {error !== undefined ? <ArrowPathIcon /> : <ArrowDownTrayIcon />}
          <SidebarLabel>{current?.status === "available" ? `Обновить до v${current.version}` : "Проверить обновления"}</SidebarLabel>
        </SidebarItem>
      )}
      {error !== undefined && <div className="px-2"><ErrorText>{error}</ErrorText></div>}
    </>
  );
}
