import { createContext, useContext } from "react";
import type { DesktopApp, Snapshot, useDesktopInstall } from "../agents/desktop_install_state";

type Observation = ReturnType<typeof useDesktopInstall>;
export type Installations = {
  observations: Record<DesktopApp, Observation>;
  jobs: DesktopApp[];
  requesting: Partial<Record<DesktopApp, boolean>>;
  errors: Partial<Record<DesktopApp, string | undefined>>;
  open: (desktop?: DesktopApp) => void;
  download: (desktop: DesktopApp) => Promise<void>;
  cancel: (desktop: DesktopApp) => Promise<void>;
  install: (desktop: DesktopApp, confirmed: boolean, close: boolean) => Promise<void>;
};

export const InstallationsContext = createContext<Installations | null>(null);

export function useInstallations() {
  const value = useContext(InstallationsContext);
  if (!value) throw new Error("InstallationsProvider is missing");
  return value;
}

export function downloadProgress(state: Snapshot | undefined) {
  return state?.status === "downloading" && state.total > 0
    ? Math.min(100, Math.floor(state.received / state.total * 100)) : undefined;
}
