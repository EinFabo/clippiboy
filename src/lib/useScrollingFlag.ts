import { useEffect, type RefObject } from "react";

/**
 * Sets `data-scrolling` on the element while it scrolls and takes it off once
 * the scrolling has rested for `restMs`.
 *
 * The stylesheet uses it to switch hit-testing off for the gallery's tiles:
 * otherwise every tile that passes under a still cursor turns on its hover
 * look and off again, and that restyling was most of the cost of a scroll.
 * One timer, re-armed only when it runs out, not on every scroll event.
 *
 * A mouse that really moves ends it at once and keeps it off until the wheel
 * or a key scrolls again: that is someone about to click, and the click should land on
 * the tile under the cursor even while the page is still gliding to a stop.
 */
export function useScrollingFlag(ref: RefObject<HTMLElement | null>, restMs = 150) {
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    let last = 0;
    let timer: number | undefined;
    /** The mouse moved since the wheel or a key last scrolled. */
    let pointing = false;
    const settle = () => {
      const left = last + restMs - performance.now();
      if (left > 0) {
        timer = window.setTimeout(settle, left);
        return;
      }
      timer = undefined;
      node.removeAttribute("data-scrolling");
    };
    const onScroll = () => {
      if (pointing) return;
      last = performance.now();
      if (timer !== undefined) return;
      node.setAttribute("data-scrolling", "");
      timer = window.setTimeout(settle, restMs);
    };
    const onMove = (event: PointerEvent) => {
      // The browser's own hover update after a scroll comes without movement.
      if (event.movementX === 0 && event.movementY === 0) return;
      pointing = true;
      if (timer === undefined) return;
      window.clearTimeout(timer);
      timer = undefined;
      node.removeAttribute("data-scrolling");
    };
    const onScrollInput = () => {
      pointing = false;
    };
    node.addEventListener("scroll", onScroll, { passive: true });
    node.addEventListener("pointermove", onMove, { passive: true });
    node.addEventListener("wheel", onScrollInput, { passive: true });
    window.addEventListener("keydown", onScrollInput);
    return () => {
      node.removeEventListener("scroll", onScroll);
      node.removeEventListener("pointermove", onMove);
      node.removeEventListener("wheel", onScrollInput);
      window.removeEventListener("keydown", onScrollInput);
      window.clearTimeout(timer);
      node.removeAttribute("data-scrolling");
    };
  }, [ref, restMs]);
}
