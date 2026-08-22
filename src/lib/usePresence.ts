import { useEffect, useRef, useState } from "react";
import { prefersReducedMotion } from "./motion";

export interface Present<T> {
  item: T;
  key: string;
  /** On its way out — it is only still here so it can be seen leaving. */
  leaving: boolean;
}

/**
 * Keeps an item around for a moment after it has been removed, so it can play
 * its exit before the DOM node goes — and keeps it in the place it had.
 *
 * The store deletes optimistically (store.ts:273): the clip is out of the array
 * before the core has even confirmed it, and without this the node would be
 * gone in the same frame the click landed.
 *
 * `hold` decides what actually deserves an exit. The gallery only holds clips
 * that were really deleted; ones that merely fell out of the current filter
 * disappear at once, otherwise every keystroke in the search field would leave
 * a trail of ghosts behind it.
 */
export function usePresence<T>(
  items: T[],
  keyOf: (item: T) => string,
  leaveMs: number,
  hold?: (key: string, item: T) => boolean,
): Array<Present<T>> {
  const known = useRef(new Map<string, T>());
  const ghosts = useRef(new Map<string, T>());
  const order = useRef<string[]>([]);
  const timers = useRef(new Map<string, number>());
  const [, bump] = useState(0);

  const live = new Map<string, T>();
  for (const item of items) live.set(keyOf(item), item);

  // Worked out while rendering, not in an effect: by the time an effect runs
  // React has already committed the list without the item, and there would be
  // no node left to animate. Writing to the ref here is safe because it only
  // ever adds what is already gone — running it twice changes nothing.
  if (!prefersReducedMotion()) {
    for (const [key, item] of known.current) {
      if (live.has(key) || ghosts.current.has(key)) continue;
      if (hold && !hold(key, item)) continue;
      ghosts.current.set(key, item);
    }
  }
  for (const key of [...ghosts.current.keys()]) {
    if (live.has(key)) ghosts.current.delete(key);
  }

  // A leaver keeps its slot: it goes back in behind whichever of its former
  // neighbours is still there. Dropping it at the end would make a deleted tile
  // jump across the gallery before it fades.
  const result: Array<Present<T>> = items.map((item) => ({
    item,
    key: keyOf(item),
    leaving: false,
  }));
  for (const [key, item] of ghosts.current) {
    result.splice(slotFor(key, order.current, result), 0, { item, key, leaving: true });
  }

  useEffect(() => {
    known.current = live;
    order.current = result.map((entry) => entry.key);

    for (const [key, timer] of timers.current) {
      // Something that came back does not need to leave any more.
      if (ghosts.current.has(key)) continue;
      clearTimeout(timer);
      timers.current.delete(key);
    }

    for (const key of ghosts.current.keys()) {
      if (timers.current.has(key)) continue;
      timers.current.set(
        key,
        window.setTimeout(() => {
          timers.current.delete(key);
          ghosts.current.delete(key);
          bump((n) => n + 1);
        }, leaveMs),
      );
    }
  });

  useEffect(() => {
    const running = timers.current;
    return () => {
      for (const timer of running.values()) clearTimeout(timer);
      running.clear();
    };
  }, []);

  return result;
}

/** Where in the new list a leaver sat, judged by the neighbour above it. */
function slotFor<T>(key: string, order: string[], result: Array<Present<T>>): number {
  const wasAt = order.indexOf(key);
  if (wasAt <= 0) return 0;
  for (let i = wasAt - 1; i >= 0; i--) {
    const anchor = result.findIndex((entry) => entry.key === order[i]);
    if (anchor !== -1) return anchor + 1;
  }
  return 0;
}
