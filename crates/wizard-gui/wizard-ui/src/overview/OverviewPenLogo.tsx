import { useId, type SVGProps } from "react";
import { ClaudeIcon, OpenAiIcon } from "../components/BrandIcons";

// The original marks are the stencil. Close, slightly bowed pen strokes retain
// their proportions and negative spaces while matching the notebook drawings.
const hatching = Array.from({ length: 56 }, (_, index) => {
  const y = -10 + index * .8;
  return `M-10 ${y}Q12 ${y + [0.18, -.12, .06][index % 3]} 34 ${y + .08}`;
}).join("");

export function OverviewPenLogo({ kind, ...props }: SVGProps<SVGSVGElement> & { kind: "claude" | "openai" }) {
  const mask = useId();
  const Icon = kind === "claude" ? ClaudeIcon : OpenAiIcon;
  return <svg viewBox="-.5 -.5 25 25" fill="none" aria-hidden="true" focusable="false" {...props}>
    <defs>
      <mask id={mask} maskUnits="userSpaceOnUse" x="-.5" y="-.5" width="25" height="25">
        <Icon width="24" height="24" fill="white" stroke="none" />
      </mask>
    </defs>
    <Icon width="24" height="24" fill="currentColor" stroke="none" opacity=".12" />
    <g mask={`url(#${mask})`}>
      <path d={hatching} transform="rotate(-32 12 12)" stroke="currentColor" strokeWidth=".55" strokeLinecap="round" />
    </g>
    <Icon width="24" height="24" fill="none" stroke="currentColor" strokeWidth=".28" strokeLinecap="round" strokeLinejoin="round" />
  </svg>;
}
