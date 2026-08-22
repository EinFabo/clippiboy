import { useLayoutEffect, useRef } from "react";
import { cn } from "@/lib/cn";

/** Options that can be slid to carry this, with their own key as value. */
export const SLIDE = "data-slide";

/**
 * The marker behind a row of options that travels to whichever one is active,
 * instead of one background switching off and another switching on.
 *
 * It measures rather than guesses, so options of different widths — "Overview"
 * next to "Audio" — work without any arithmetic at the call site. Put it as a
 * direct child of the row, give the row `relative`, and give every option
 * `data-slide` with its key.
 *
 * The row is found through the marker's own parent rather than through a ref
 * handed in from outside: React attaches a parent's ref only after its
 * children's layout effects have run, so on the first pass such a ref is still
 * empty — and a row whose selection never changes would then never get its
 * marker at all.
 */
export function SlidingIndicator({
  activeKey,
  className,
}: {
  activeKey: string;
  className?: string;
}) {
  const box = useRef<HTMLSpanElement>(null);
  const placed = useRef(false);

  useLayoutEffect(() => {
    const pill = box.current;
    const root = pill?.parentElement;
    if (!pill || !root) return;

    // Compared rather than selected: keys come from monitor ids and durations,
    // and a selector would have to escape whatever those happen to contain.
    const target = [...root.querySelectorAll<HTMLElement>(`[${SLIDE}]`)].find(
      (node) => node.getAttribute(SLIDE) === activeKey,
    );
    if (!target) return;

    const place = () => {
      pill.style.width = `${target.offsetWidth}px`;
      pill.style.height = `${target.offsetHeight}px`;
      pill.style.transform = `translate(${target.offsetLeft}px, ${target.offsetTop}px)`;
      pill.style.opacity = "1";
    };

    if (placed.current) {
      place();
    } else {
      // The first placement is not a move — it is where the marker has always
      // been. Sliding in from the corner on load would be noise.
      pill.style.transition = "none";
      place();
      void pill.offsetWidth;
      pill.style.transition = "";
      placed.current = true;
    }

    // Labels can change width (a game name in a filter, a resolution switching
    // to four digits) and the window can be resized. Both move the target
    // without anything re-rendering.
    const observer = new ResizeObserver(place);
    observer.observe(target);
    observer.observe(root);
    return () => observer.disconnect();
  }, [activeKey]);

  return (
    <span
      ref={box}
      aria-hidden
      style={{ opacity: 0 }}
      className={cn(
        "pointer-events-none absolute top-0 left-0",
        "transition-[transform,width] duration-[260ms] ease-[var(--ease-out-soft)]",
        className,
      )}
    />
  );
}
