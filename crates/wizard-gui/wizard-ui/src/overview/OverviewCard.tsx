import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Subheading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";

/** Shared card anatomy keeps headings and actions aligned across overview steps. */
export function OverviewCardContent({ title, description, descriptionId, descriptionTitle, actions, view, headingLevel = 3 }: {
  title: string;
  description: ReactNode;
  descriptionId?: string;
  descriptionTitle?: string;
  actions: ReactNode;
  view?: string;
  headingLevel?: 2 | 3;
}) {
  return <div className="overview-card-body">
    <div className="overview-card-copy" key={view}>
      <Subheading level={headingLevel}>{title}</Subheading>
      <Text className="mt-2" id={descriptionId} title={descriptionTitle}>{description}</Text>
    </div>
    <div className="overview-card-actions">{actions}</div>
  </div>;
}

// Remember a completed intro for this app launch, including page navigation.
// A new launch/reload gets a fresh first hover; interrupted runs do not count.
let introSeen = false;

// The marker hatches the card, clears along the same strokes, then circles
// the action counterclockwise. The pen lifts between the two gestures.
const strike = "M-34 128 C8 94 61 31 77 25 C96 17 47 111 14 182 C-9 230 0 237 31 207 L193 27 C229 -12 222 18 196 59 L49 281 C16 331 20 337 58 294 L276 66 C308 33 315 45 289 88 L124 347 C100 384 113 389 146 352 L327 150 C361 113 366 131 342 171 L233 353 C209 392 220 396 249 367 L356 276";

export function OverviewCard({ kind, disabled, activationReady = true, suspended = false, onboarding = true, children }: {
  kind: "account" | "install" | "configure";
  disabled: boolean;
  activationReady?: boolean;
  suspended?: boolean;
  onboarding?: boolean;
  children: ReactNode;
}) {
  const card = useRef<HTMLElement>(null);
  const path = useRef<SVGPathElement>(null);
  const animation = useRef<Animation | null>(null);
  const previousDisabled = useRef<boolean | undefined>(undefined);
  const [inkVisible, setInkVisible] = useState(false);
  const [outlinePending, setOutlinePending] = useState(false);
  const [buttonVisible, setButtonVisible] = useState(false);

  useEffect(() => {
    if (disabled || !onboarding) {
      animation.current?.cancel();
      setInkVisible(false);
      setOutlinePending(false);
    }
  }, [disabled, onboarding]);

  useEffect(() => {
    let previousSize: { width: number; height: number } | undefined;
    const observer = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      if (previousSize && (previousSize.width !== width || previousSize.height !== height)) {
        animation.current?.cancel(); setInkVisible(false);
      }
      previousSize = { width, height };
    });
    if (card.current) observer.observe(card.current);
    return () => { observer.disconnect(); animation.current?.cancel(); };
  }, []);

  useEffect(() => {
    const button = card.current?.querySelector(".overview-card-actions > button");
    const observer = new IntersectionObserver(([entry]) => setButtonVisible(entry.isIntersecting && entry.intersectionRatio >= .95), { threshold: [.95] });
    if (button) observer.observe(button);
    return () => observer.disconnect();
  }, []);

  // Loading the existing account/agent state establishes a baseline. Only a
  // real locked -> available transition requests a new-step hint.
  useEffect(() => {
    if (!onboarding || kind === "account" || !activationReady) return;
    if (previousDisabled.current === true && !disabled) setOutlinePending(true);
    if (disabled) setOutlinePending(false);
    previousDisabled.current = disabled;
  }, [kind, disabled, activationReady, onboarding]);

  const play = useCallback((mode: "intro" | "outline") => {
    const element = path.current;
    const button = card.current?.querySelector<HTMLButtonElement>(".overview-card-actions > button");
    if (!onboarding || disabled || !element || !button || animation.current?.playState === "running"
      || window.matchMedia("(prefers-reduced-motion: reduce)").matches
      || document.documentElement.dataset.motion === "reduce") return false;

    animation.current?.cancel();
    // Work in CSS pixels so the trail and button outline also align in WebKit
    // and in the horizontal card layout, without scaled-dash rounding artifacts.
    const bounds = element.ownerSVGElement!.getBoundingClientRect();
    const target = button.getBoundingClientRect();
    const w = bounds.width, h = bounds.height;
    const x = target.left - bounds.left, y = target.top - bounds.top;
    const bw = target.width, bh = target.height;
    const cx = x + bw / 2, cy = y + bh / 2;
    const rx = bw / 2 + 11, ry = bh / 2 + 13;
    let coordinate = 0;
    const scribble = strike.replace(/-?\d+(?:\.\d+)?/g, (value) => String(Number(value) * (coordinate++ % 2 === 0 ? w / 320 : h / 360)));
    // Move the pen invisibly to a rounded, slightly uneven loop with a short
    // overlap across the top, rather than a button-shaped frame.
    const penDown = `L${cx + rx * .27} ${cy - ry * .97}`;
    const circle = `C${cx - rx * .25} ${cy - ry * 1.08} ${cx - rx * .94} ${cy - ry * .8} ${cx - rx * 1.02} ${cy - ry * .12}
      C${cx - rx * 1.1} ${cy + ry * .57} ${cx - rx * .58} ${cy + ry * 1.07} ${cx + rx * .1} ${cy + ry * .99}
      C${cx + rx * .73} ${cy + ry * .98} ${cx + rx * 1.1} ${cy + ry * .53} ${cx + rx * .98} ${cy - ry * .14}
      C${cx + rx * .91} ${cy - ry * .86} ${cx + rx * .38} ${cy - ry * 1.12} ${cx - rx * .16} ${cy - ry * .95}`;
    const measure = (d: string) => { element.setAttribute("d", d); return element.getTotalLength(); };
    const drawn = mode === "intro" ? measure(scribble) : 0;
    const arrived = mode === "intro" ? measure(scribble + penDown) : 0;
    const total = measure(mode === "intro" ? scribble + penDown + circle : penDown.replace(/^L/, "M") + circle);
    const frame = (offset: number, head: number, tail: number, width: number, easing = "linear") => ({
      offset, easing, strokeDasharray: `${Math.max(.01, head - tail)} ${total + 1}`,
      strokeDashoffset: -tail, strokeWidth: width,
    });

    const fullSequence = [
      frame(0, 0, 0, 32, "cubic-bezier(.3,.05,.6,.95)"),
      frame(.28, drawn, 0, 32, "cubic-bezier(.4,0,.6,1)"),
      frame(.46, drawn, drawn, 28),
      frame(.461, drawn, drawn, 0),
      frame(.48, arrived, arrived, 0),
      frame(.485, arrived, arrived, 3, "cubic-bezier(.2,.25,.35,1)"),
      frame(.66, total, arrived, 8.5),
      frame(.83, total, arrived, 9, "cubic-bezier(.4,0,.6,1)"),
      frame(1, total, total, .1),
    ];
    const outlineSequence = [
      frame(0, 0, 0, 3, "cubic-bezier(.2,.25,.35,1)"),
      frame(.325, total, 0, 8.5),
      frame(.687, total, 0, 9, "cubic-bezier(.4,0,.6,1)"),
      frame(1, total, total, .1),
    ];
    animation.current = element.animate(mode === "intro" ? fullSequence : outlineSequence,
      { duration: mode === "intro" ? 2200 : 1162, fill: "forwards" });
    animation.current.onfinish = () => {
      if (mode === "intro") introSeen = true;
      setInkVisible(false);
    };
    setInkVisible(true);
    return true;
  }, [disabled, onboarding]);

  useEffect(() => {
    if (!onboarding || !outlinePending || disabled || suspended || !buttonVisible) return;
    let frame: number | undefined;
    const showWhenVisible = () => {
      if (document.visibilityState !== "visible") return;
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        play("outline");
        setOutlinePending(false);
      });
    };
    showWhenVisible();
    document.addEventListener("visibilitychange", showWhenVisible);
    return () => {
      if (frame !== undefined) cancelAnimationFrame(frame);
      document.removeEventListener("visibilitychange", showWhenVisible);
    };
  }, [onboarding, outlinePending, disabled, suspended, buttonVisible, play]);

  function playIntroOnce() {
    if (onboarding && kind === "account" && !suspended && !introSeen) play("intro");
  }

  return <article ref={card} className="overview-card" data-kind={kind} data-disabled={disabled} data-onboarding={onboarding} data-ink-visible={inkVisible}
    onPointerEnter={(event) => {
      if (event.pointerType !== "touch") playIntroOnce();
    }}
    onFocus={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget) && event.target.matches(":focus-visible")) playIntroOnce();
    }}
    onClickCapture={() => { animation.current?.cancel(); setInkVisible(false); }}>
    {children}
    {onboarding && <svg className="overview-card-strike" aria-hidden="true" focusable="false">
      <path ref={path} />
    </svg>}
  </article>;
}
