import { useEffect } from "react";
import { AppLogo } from "./AppLogo";
import "./StartupScreen.css";

export function StartupScreen({ onComplete }: { onComplete: () => void }) {
  useEffect(() => {
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    if (motion.matches || document.documentElement.dataset.motion === "reduce") { onComplete(); return; }
    // A timer also finishes startup if the animation is disabled or interrupted.
    const timer = window.setTimeout(onComplete, 1400);
    const skipAnimation = () => { if (motion.matches) onComplete(); };
    motion.addEventListener("change", skipAnimation);
    return () => {
      window.clearTimeout(timer);
      motion.removeEventListener("change", skipAnimation);
    };
  }, [onComplete]);

  return <main className="startup-screen" aria-label="Запускаем Wizard" aria-busy="true">
    <AppLogo className="startup-logo" />
  </main>;
}
