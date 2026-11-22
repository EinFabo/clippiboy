import { api, inTauri } from "./ipc";
import { CONTROLLERS_MAX_AGE_MS, isStale } from "./settingsData";
import type { Controllers } from "./types";

let last: { value: Controllers; at: number } | null = null;

/** The last answer, as long as it is recent enough to stand as current. */
export function cachedControllers(): Controllers | null {
  return last && !isStale(last.at, Date.now(), CONTROLLERS_MAX_AGE_MS) ? last.value : null;
}

export function rememberControllers(value: Controllers) {
  last = { value, at: Date.now() };
}

let pending: Promise<Controllers | null> | null = null;

/** One question at a time: the prefetch and the Hotkeys tab asking together share an answer. */
export function askControllers(): Promise<Controllers | null> {
  pending ??= api
    .watchControllers()
    .then((value) => {
      rememberControllers(value);
      return value;
    })
    .catch(() => null)
    .finally(() => {
      pending = null;
    });
  return pending;
}

/** Asked when Settings opens, so the Hotkeys tab finds an answer waiting. */
export function prefetchControllers() {
  if (!inTauri) return;
  void askControllers();
}
