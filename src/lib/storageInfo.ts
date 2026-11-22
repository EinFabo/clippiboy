import { create } from "zustand";
import { api, inTauri } from "./ipc";
import {
  isStale,
  parseStorageCache,
  readStillCurrent,
  rerunOwed,
  STORAGE_CACHE_KEY,
  STORAGE_MAX_AGE_MS,
  type StorageCache,
} from "./settingsData";
import type { StorageUsage, TrimOriginals } from "./types";

interface StorageInfo extends StorageCache {
  /** Reads what is missing or stale; `force` reads the numbers again regardless. */
  prefetch: (force?: boolean) => void;
  setTrims: (trims: TrimOriginals) => void;
  /** The data changed: what is shown stays, but counts as stale and in-flight reads are dropped. */
  invalidate: () => void;
}

function load(): StorageCache {
  try {
    const cached = parseStorageCache(localStorage.getItem(STORAGE_CACHE_KEY));
    if (cached) return cached;
  } catch {
    /* blocked storage: the numbers simply start unknown */
  }
  return { usage: null, trims: null, defaultDir: null, at: null };
}

function save({ usage, trims, defaultDir, at }: StorageCache) {
  try {
    localStorage.setItem(STORAGE_CACHE_KEY, JSON.stringify({ usage, trims, defaultDir, at }));
  } catch {
    /* the numbers then last until the window closes */
  }
}

let usageBusy = false;
let trimsBusy = false;
let dirBusy = false;
/** Counts invalidations; a read that started before the last one is dropped. */
let epoch = 0;
/** A forced read arrived while one was running: read once more after it. */
let usageAgain = false;
let trimsAgain = false;

/**
 * What the Storage tab shows, kept across visits: the last answer is on screen
 * at once and read again behind it. The usage walk covers every file, so it
 * must not start from the tab itself.
 */
export const useStorageInfo = create<StorageInfo>((set, get) => {
  const keep = (patch: Partial<StorageCache>) => {
    set(patch);
    save(get());
  };
  return {
    ...load(),
    prefetch(force = false) {
      if (!inTauri) return;
      if (force) epoch++;
      const s = get();
      const stale = isStale(s.at, Date.now(), STORAGE_MAX_AGE_MS);
      const readUsage = () => {
        usageBusy = true;
        const started = epoch;
        api
          .storageUsage()
          .then((usage: StorageUsage) => {
            if (readStillCurrent(started, epoch)) keep({ usage, at: Date.now() });
          })
          .catch(() => {
            // An unreadable folder is a missing number, not an error.
          })
          .finally(() => {
            usageBusy = false;
            if (usageAgain) {
              usageAgain = false;
              readUsage();
            }
          });
      };
      const readTrims = () => {
        trimsBusy = true;
        const started = epoch;
        api
          .trimOriginals()
          .then((trims) => {
            if (readStillCurrent(started, epoch)) keep({ trims });
          })
          .catch(() => {
            if (!get().trims) keep({ trims: { count: 0, bytes: 0 } });
          })
          .finally(() => {
            trimsBusy = false;
            if (trimsAgain) {
              trimsAgain = false;
              readTrims();
            }
          });
      };
      if (force || stale) {
        if (usageBusy) usageAgain ||= force;
        else readUsage();
      }
      if (force || stale || !s.trims) {
        if (trimsBusy) trimsAgain ||= force;
        else readTrims();
      }
      // The same all session: asked once.
      if (!dirBusy && !s.defaultDir) {
        dirBusy = true;
        api
          .defaultClipDir()
          .then((defaultDir) => keep({ defaultDir }))
          .catch(() => {})
          .finally(() => {
            dirBusy = false;
          });
      }
    },
    setTrims(trims) {
      keep({ trims });
    },
    invalidate() {
      epoch++;
      // The read in flight is dropped; a burst of changes owes one more walk, not one each.
      usageAgain = rerunOwed(usageBusy, usageAgain);
      trimsAgain = rerunOwed(trimsBusy, trimsAgain);
      keep({ at: null });
    },
  };
});
