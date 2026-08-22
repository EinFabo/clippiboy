import { useLayoutEffect, useRef, type RefObject } from "react";
import { EASE_OUT_SOFT, prefersReducedMotion } from "./motion";

/** Children that take part carry this attribute, with a stable id as value. */
const KEY = "data-flip";

/** A child's place in the layout, before anything was transformed. */
interface Spot {
  left: number;
  top: number;
}

/**
 * Lets a list re-order itself without anything jumping.
 *
 * The trick is old and cheap: remember where every child stood, let the browser
 * lay the list out anew, then push each child back to its old place with a
 * transform and let it travel to the new one. Nothing but `transform` moves, so
 * a gallery of twenty tiles costs the same as one.
 *
 * `signature` is what tells the hook that the list changed — pass the joined
 * ids, not the array. The gallery rebuilds its `visible` array on every
 * keystroke, and comparing arrays by identity would restart every animation
 * for a search that did not actually change the result.
 */
export function useFlip(
  container: RefObject<HTMLElement | null>,
  signature: string,
  duration = 260,
) {
  const before = useRef(new Map<string, Spot>());
  const running = useRef(new Map<string, Animation>());

  useLayoutEffect(() => {
    const root = container.current;
    if (!root) return;

    const nodes = [...root.querySelectorAll<HTMLElement>(`[${KEY}]`)];

    // How far a running animation has already carried each child. Read before
    // they are cancelled: a second change mid-flight would otherwise restart
    // from a place the child left long ago, and that shows as a jolt.
    const carried = new Map<string, [number, number]>();
    for (const node of nodes) {
      const key = node.getAttribute(KEY);
      if (key && running.current.get(key)?.playState === "running") {
        carried.set(key, translationOf(node));
      }
    }
    for (const animation of running.current.values()) animation.cancel();
    running.current.clear();

    // `offsetLeft`/`offsetTop` rather than `getBoundingClientRect`: those are
    // layout, untouched by any transform. A tile still playing its arrival, or
    // a gallery that was simply scrolled, would otherwise measure as a move and
    // get animated for nothing.
    const after = new Map<string, Spot>();
    for (const node of nodes) {
      const key = node.getAttribute(KEY);
      if (key) after.set(key, { left: node.offsetLeft, top: node.offsetTop });
    }

    if (!prefersReducedMotion()) {
      for (const node of nodes) {
        const key = node.getAttribute(KEY);
        const from = key ? before.current.get(key) : undefined;
        const to = key ? after.get(key) : undefined;
        if (!key || !from || !to) continue;

        const [carriedX, carriedY] = carried.get(key) ?? [0, 0];
        const dx = from.left + carriedX - to.left;
        const dy = from.top + carriedY - to.top;
        // A shift of less than a pixel is not worth an animation.
        if (Math.abs(dx) < 1 && Math.abs(dy) < 1) continue;

        const animation = node.animate(
          [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
          // Added to whatever is already there, so a tile that is still
          // playing its arrival keeps it and simply travels while it does.
          { duration, easing: EASE_OUT_SOFT, composite: "add" },
        );
        running.current.set(key, animation);
        // No fill: once it arrives the stylesheet takes over again.
        void animation.finished
          .then(() => {
            if (running.current.get(key) === animation) running.current.delete(key);
          })
          .catch(() => {});
      }
    }

    before.current = after;
    // Deliberately no cleanup that cancels: React runs it before the next
    // effect, and the offsets above would then always read zero.
  }, [container, signature, duration]);
}

/**
 * How far a running animation has already carried the element. Reads the
 * composed value, so during an arrival it also picks up that animation's own
 * few pixels — close enough for the one case it matters in, a move that is
 * interrupted mid-flight.
 */
function translationOf(node: HTMLElement): [number, number] {
  const value = getComputedStyle(node).transform;
  if (!value || value === "none") return [0, 0];
  const matrix = new DOMMatrixReadOnly(value);
  return [matrix.m41, matrix.m42];
}
