import { useState } from "react";
import { GlobeAltIcon } from "@heroicons/react/24/outline";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { ActionBar } from "../components/ui/Layout";
import { useEnableSystemProxy, useSystemProxy } from "../settings/system_proxy";

export const systemProxyBannerDismissedKey = "coreinfra.system-proxy-banner.dismissed";

export function SystemProxyBanner() {
  const [dismissed, setDismissed] = useState(() => {
    try {
      return localStorage.getItem(systemProxyBannerDismissedKey) === "true";
    } catch {
      return false;
    }
  });
  const { state, error: loadError } = useSystemProxy(!dismissed);
  const { enable, submitting, error } = useEnableSystemProxy();
  const canEnable =
    state?.status === "off" && !state.busy && !state.conflict && !state.read_error && !loadError;

  function dismiss() {
    setDismissed(true);
    try {
      localStorage.setItem(systemProxyBannerDismissedKey, "true");
    } catch {
      /* The current view still honors the choice when storage is unavailable. */
    }
  }

  if (dismissed || (!canEnable && !submitting && !error)) return null;

  return (
    <section className="overview-proxy-banner" aria-labelledby="overview-proxy-title">
      <div className="overview-proxy-message">
        <GlobeAltIcon className="overview-proxy-icon" aria-hidden="true" />
        <Heading level={2} id="overview-proxy-title">
          Для доступа к OpenAI и Anthropic из браузера{" "}
          <span className="inline-block max-w-full">включите системный прокси.</span>
        </Heading>
        <Text className="col-start-2">Все остальные ресурсы будут доступны напрямую.</Text>
      </div>
      <ActionBar className="overview-proxy-actions">
        <Button plain className="overview-proxy-skip" disabled={submitting} onClick={dismiss}>Пропустить</Button>
        <Button
          color="blue"
          disabled={!canEnable || submitting}
          aria-busy={submitting}
          onClick={async () => {
            if (await enable()) dismiss();
          }}
        >
          {submitting ? "Включаем…" : "Включить"}
        </Button>
      </ActionBar>
      {error && <div role="alert" className="basis-full"><ErrorText>{error}</ErrorText></div>}
    </section>
  );
}
