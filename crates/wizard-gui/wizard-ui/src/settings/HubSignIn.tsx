import { useCallback, useEffect, useState } from "react";
import { useReducedMotion } from "motion/react";
import { Button } from "../components/catalyst/button";
import { Link } from "../components/catalyst/link";
import { ErrorText } from "../components/ErrorText";
import type { HubAccountControl } from "./HubAccount";
import "./HubSignIn.css";

// The CSS sequence finishes at 3.5s. Focus or a click can finish it immediately.
const INTRO_DURATION_MS = 3500;

// Keep the first step visible even if agent detection is loading or fails.
export function HubSignIn({ hub }: { hub: HubAccountControl }) {
  const reducedMotion = useReducedMotion();
  const [introduced, setIntroduced] = useState(false);
  const finishIntro = useCallback(() => setIntroduced(true), []);
  const accountLoaded = hub.account !== undefined;
  const pending = hub.account?.pending;
  const animateIntro = !introduced && !reducedMotion && !pending && !hub.error;

  useEffect(() => {
    if (introduced) return;
    if (hub.signedIn || reducedMotion || pending || hub.error) {
      finishIntro();
      return;
    }
    if (!accountLoaded) return;
    const timer = window.setTimeout(finishIntro, INTRO_DURATION_MS);
    return () => window.clearTimeout(timer);
  }, [accountLoaded, introduced, hub.signedIn, hub.error, pending, reducedMotion, finishIntro]);

  // Do not flash the welcome animation while restoring a saved account.
  if (hub.signedIn || (!accountLoaded && !hub.error)) return null;

  return (
    <section aria-label="Вход в CoreInfra AI Hub"
      data-intro={animateIntro ? "" : undefined}
      onPointerDownCapture={finishIntro} onFocusCapture={finishIntro}
      className="hub-onboarding mb-10 rounded-3xl border border-zinc-950/5 px-6 py-7 text-left sm:p-8 dark:border-white/10">
      <div className="hub-onboarding-symbol" aria-hidden="true">
        <div className="hub-onboarding-dots"><span /><span /><span /></div>
        <div className="hub-onboarding-mark">
          <div className="hub-onboarding-mark-gray" />
          <div className="hub-onboarding-mark-color" />
        </div>
      </div>
      <div className="hub-onboarding-reveal">
        <div className="hub-onboarding-content">
          <div aria-live="polite">
            <h2 className="hub-onboarding-title text-2xl/8 font-semibold tracking-tight text-zinc-950 dark:text-white">
              {pending ? "Завершите вход в браузере" : "Ура!"}
            </h2>
            <p className="hub-onboarding-copy mt-3 max-w-lg text-sm/6 text-zinc-600 dark:text-zinc-400">
              {pending
                ? "После входа вернитесь в Wizard — аккаунт подключится автоматически."
                : <>Wizard установлен, теперь войдите в аккаунт хаба, если аккаунта нет – <Link
                    href="https://hub.coreinfra.ai/register" target="_blank" rel="noreferrer"
                    className="text-zinc-800 underline decoration-zinc-400 underline-offset-2 hover:decoration-current focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-500 dark:text-zinc-200 dark:decoration-zinc-500"
                    aria-disabled={hub.submitting || undefined}
                    onClick={(event) => { event.preventDefault(); hub.register(); }}
                  >создайте его</Link>, для этого потребуется всего лишь нажать одну кнопку.</>}
            </p>
          </div>
          <div className="hub-onboarding-action mt-5">
            <Button color="blue" disabled={!hub.account || hub.submitting || hub.agentBusy} onClick={hub.change}>
              {hub.submitting ? "Открываем браузер…" : pending ? "Продолжить вход" : "Войти в CoreInfra Hub"}
            </Button>
          </div>
          {hub.error && <div role="alert" className="mt-4"><ErrorText>{hub.error}</ErrorText></div>}
        </div>
      </div>
    </section>
  );
}
