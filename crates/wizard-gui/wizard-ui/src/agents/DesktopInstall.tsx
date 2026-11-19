import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "../components/catalyst/button";
import { ErrorText } from "../components/ErrorText";
import {
  useInstallations,
} from "../onboarding/installations_state";
import type { DesktopApp } from "./desktop_install_state";

export type { DesktopApp } from "./desktop_install_state";

/** Archive cancellation is available only in the expanded installation panel. */
export function CancelDownloadButton({
  desktop,
  size = "md",
  disabled = false,
}: {
  desktop: DesktopApp;
  size?: "sm" | "md";
  disabled?: boolean;
}) {
  const controls = useInstallations();
  const requesting = Boolean(controls.requesting[desktop]);
  const [cancelling, setCancelling] = useState(false);
  return (
    <Button
      plain
      size={size}
      disabled={disabled || requesting}
      onClick={async (event) => {
        event.stopPropagation();
        setCancelling(true);
        try {
          await controls.cancel(desktop);
        } finally {
          setCancelling(false);
        }
      }}
    >
      {cancelling ? "Отменяем…" : "Отменить"}
    </Button>
  );
}

/** A stable version column and action group, shared by both desktop agents. */
export function DesktopVersion({
  desktop,
  installed,
  signedIn,
  disabled,
  version,
  children,
}: {
  desktop: DesktopApp;
  installed: boolean;
  signedIn: boolean;
  disabled: boolean;
  version: string | null;
  children: ReactNode;
}) {
  const controls = useInstallations();
  const observation = controls.observations[desktop];
  const { state } = observation;
  const displayedVersion = state?.status === "installed" ? state.version : version;
  const previousVersion = useRef(displayedVersion);
  const versionLabel = useRef<HTMLSpanElement>(null);
  useLayoutEffect(() => {
    const previous = previousVersion.current;
    previousVersion.current = displayedVersion;
    if (!previous || !displayedVersion || previous === displayedVersion ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches ||
      document.documentElement.dataset.motion === "reduce") return;
    const animation = versionLabel.current?.animate?.([
      { opacity: .25, transform: "translateY(6px)" },
      { opacity: 1, transform: "translateY(0)" },
    ], { duration: 280, easing: "ease-out" });
    return () => animation?.cancel();
  }, [displayedVersion]);
  if (observation.hidden) return <>{children}</>;
  if (!state)
    return (
      <>
        <span>{children}</span>
        <ErrorText>{observation.error}</ErrorText>
      </>
    );
  const current = state.status === "up_to_date" || state.status === "installed";
  const checking = state.status === "idle" || state.status === "checking";
  const checkFailed = state.status === "failed" && state.operation === "check";
  return (
    <div className="ui-version-row">
      <div className="ui-version-copy">
        <span ref={versionLabel} className="ui-version-value" aria-live="polite">{displayedVersion ? `v${displayedVersion}` : children}</span>
      </div>
      <div className="ui-version-actions">
        {checking && signedIn && (
          <span role="status" className="ui-version-status">
            Проверяем версию…
          </span>
        )}
        {checkFailed && (
          <span title={state.message} className="ui-version-status">
            Не удалось проверить версию
          </span>
        )}
        {!current &&
          !checkFailed &&
          (!checking || (!installed && !signedIn)) && (
            <Button
              outline
              size="sm"
              disabled={disabled}
              onClick={() => controls.open(desktop)}
            >
              {installed ? "Обновить" : "Скачать"}
            </Button>
          )}
      </div>
      {controls.errors[desktop] && (
        <div className="ui-version-error">
          <ErrorText>{controls.errors[desktop]}</ErrorText>
        </div>
      )}
    </div>
  );
}
