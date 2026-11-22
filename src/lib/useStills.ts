import { useCallback, useEffect, useRef, useState } from "react";
import { isHidden } from "./hidden";
import { api, inTauri } from "./ipc";
import { mockThumbs } from "./mock";
import { STILL_EVERY_MS, shouldFetchStills, stillsById } from "./stills";

/**
 * One `capture_thumbs` at a time for the whole page, however many maps ask:
 * each request waits for the one before.
 */
let line: Promise<unknown> = Promise.resolve();
function inLine<T>(call: () => Promise<T>): Promise<T> {
  const next = line.then(call, call);
  line = next.catch(() => {});
  return next;
}

/**
 * Still images of the screens in `ids`, fresh every few seconds while the
 * component is mounted, the page can be seen and the window has the focus:
 * screen id → image URL, screens without one left out. Without the focus the
 * last stills stay as they are.
 *
 * Asked for at once when it mounts, when `ids` change, when `since` changes
 * (a new target list) and when the page comes back into view or into focus. A request
 * still out skips the timer's tick; one asked for at once waits for it and
 * then goes.
 */
export function useStills(ids: string[], since: unknown): Record<string, string> {
  const [stills, setStills] = useState<Record<string, string>>({});
  const asked = useRef(ids);
  const run = useRef({ alive: false, inFlight: false, queued: false });
  const key = ids.join("\n");

  const fetchNow = useCallback((urgent: boolean) => {
    const state = run.current;
    const visible = document.visibilityState === "visible" && !isHidden();
    const focused = document.hasFocus();
    const screens = asked.current;
    if (!state.alive) return;
    if (!shouldFetchStills({ visible, focused, inFlight: state.inFlight, screens: screens.length })) {
      if (state.inFlight && urgent) state.queued = true;
      return;
    }
    state.inFlight = true;
    const call = inLine(() =>
      inTauri ? api.captureThumbs(screens) : Promise.resolve(mockThumbs(screens)),
    );
    call
      .then((thumbs) => {
        if (state.alive) setStills(stillsById(thumbs));
      })
      // The last stills stay; the next tick tries again.
      .catch(() => {})
      .finally(() => {
        state.inFlight = false;
        if (state.queued) {
          state.queued = false;
          fetchNow(true);
        }
      });
  }, []);

  useEffect(() => {
    const state = run.current;
    state.alive = true;
    const timer = window.setInterval(() => fetchNow(false), STILL_EVERY_MS);
    const onSeen = () => {
      if (document.visibilityState === "visible") fetchNow(true);
    };
    document.addEventListener("visibilitychange", onSeen);
    // Back from the tray the window comes with focus (`hidden.ts`).
    window.addEventListener("focus", onSeen);
    return () => {
      state.alive = false;
      state.queued = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onSeen);
      window.removeEventListener("focus", onSeen);
    };
  }, [fetchNow]);

  useEffect(() => {
    asked.current = key ? key.split("\n") : [];
    fetchNow(true);
  }, [key, since, fetchNow]);

  return stills;
}
