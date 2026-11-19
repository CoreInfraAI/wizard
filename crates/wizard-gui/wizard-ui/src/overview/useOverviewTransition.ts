import { useLayoutEffect, useRef, useState, type RefObject } from "react";

/** First setup folds its installation card into the persistent management action. */
export function useOverviewTransition(root: RefObject<HTMLDivElement | null>, {
  ready, connected, requested, suspended,
}: { ready: boolean; connected: boolean; requested: boolean; suspended: boolean }) {
  const [dashboard, setDashboard] = useState<boolean>();
  const [merging, setMerging] = useState(false);
  const pending = useRef<{ ghost: HTMLElement; source: DOMRect; cards: Map<string, DOMRect> } | undefined>(undefined);
  const animations = useRef<Animation[]>([]);

  useLayoutEffect(() => {
    if (!ready) return;
    if (dashboard === undefined) { setDashboard(requested); return; }
    if (!connected) { setDashboard(false); return; }
    if (dashboard || !requested || suspended) return;
    const source = root.current?.querySelector<HTMLElement>('[data-kind="install"]');
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches ||
      document.documentElement.dataset.motion === "reduce";
    if (source && !reduced && typeof source.animate === "function") {
      const bounds = source.getBoundingClientRect();
      if (bounds.width > 0 && bounds.top < innerHeight && bounds.bottom > 0) {
        const ghost = source.cloneNode(true) as HTMLElement;
        ghost.inert = true;
        ghost.setAttribute("aria-hidden", "true");
        ghost.classList.add("overview-merge-ghost");
        Object.assign(ghost.style, { left: `${bounds.left}px`, top: `${bounds.top}px`, width: `${bounds.width}px`, height: `${bounds.height}px` });
        const cards = new Map<string, DOMRect>();
        root.current?.querySelectorAll<HTMLElement>('.overview-hero, .overview-card').forEach(card => {
          cards.set(card.dataset.kind ?? "hero", card.getBoundingClientRect());
        });
        pending.current = { ghost, source: bounds, cards };
        setMerging(true);
      }
    }
    setDashboard(true);
  }, [ready, connected, requested, suspended, dashboard, root]);

  useLayoutEffect(() => {
    const transition = pending.current;
    const target = root.current?.querySelector<HTMLElement>('[data-management-target]');
    if (!dashboard || !transition || !target) return;
    pending.current = undefined;
    const { ghost, source, cards } = transition;
    // The balance notice can push the destination just below the fold. Reveal
    // it before measuring; FLIP also carries the banner through the scroll.
    const initialDestination = target.getBoundingClientRect();
    if (initialDestination.bottom > innerHeight || initialDestination.top < 0) {
      if (initialDestination.bottom > innerHeight * 1.5 || initialDestination.top < -innerHeight * .5) {
        setMerging(false);
        return;
      }
      target.scrollIntoView({ block: "nearest" });
    }
    const destination = target.getBoundingClientRect();
    const dx = destination.left - source.left, dy = destination.top - source.top;
    const sx = destination.width / source.width, sy = destination.height / source.height;
    document.body.appendChild(ghost);
    const animation = ghost.animate([
      { transform: "translate(0, 0) scale(1)", opacity: 1, clipPath: "polygon(0 0, 100% 0, 100% 100%, 0 100%)", offset: 0 },
      { transform: `translate(${dx * .35}px, ${dy * .28}px) scale(.78, .94)`, opacity: .96, clipPath: "polygon(4% 0, 96% 0, 76% 100%, 24% 100%)", offset: .35 },
      { transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`, opacity: .7, clipPath: "polygon(0 0, 100% 0, 100% 100%, 0 100%)", offset: .86 },
      { transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`, opacity: 0, offset: 1 },
    ], { duration: 850, easing: "cubic-bezier(.22,.7,.18,1)", fill: "forwards" });
    animations.current.push(animation);
    animation.onfinish = () => { ghost.remove(); setMerging(false); };
    animation.oncancel = () => ghost.remove();
    root.current?.querySelectorAll<HTMLElement>('.overview-hero, .overview-card').forEach(card => {
      const before = cards.get(card.dataset.kind ?? "hero");
      if (!before) return;
      const after = card.getBoundingClientRect();
      animations.current.push(card.animate([
        { transform: `translate(${before.left - after.left}px, ${before.top - after.top}px) scale(${before.width / after.width}, 1)` },
        { transform: "none" },
      ], { duration: 650, easing: "cubic-bezier(.22,.7,.18,1)" }));
    });
    animations.current.push(target.animate([
      { transform: "scale(1)" }, { transform: "scale(.94)" },
      { transform: "scale(1.035)" }, { transform: "scale(1)" },
    ], { delay: 600, duration: 440, easing: "ease-out" }));
    return () => { animation.cancel(); ghost.remove(); };
  }, [dashboard, root]);

  useLayoutEffect(() => () => {
    animations.current.forEach(animation => animation.cancel());
    pending.current?.ghost.remove();
  }, []);

  return { dashboard: dashboard ?? requested, merging };
}
