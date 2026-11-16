import clsx from "clsx";
import wizardLogo from "../assets/logo-wizard.svg";

export function AppLogo({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox="0 0 1145 256"
      fill="none"
      className={clsx(className, "[--logo-lettering:#252298] [--logo-mark:#112FC4] [--logo-accent:#1E3FE3] dark:[--logo-lettering:#A5B4FC] dark:[--logo-mark:#6174EF] dark:[--logo-accent:#8594FF]")}
    >
      <use href={`${wizardLogo}#logo`} />
    </svg>
  );
}
