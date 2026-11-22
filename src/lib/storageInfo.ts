import { create } from "zustand";
import { api, inTauri } from "./ipc";
import {
  isStale,
  parseStorageCache,
  STORAGE_CACHE_KEY,
  STORAGE_MAX_AGE_MS,
  type StorageCache,
} from "./settingsData";
import type { StorageUsage, TrimOriginals } from "./types";

interface StorageInfo extends StorageCache {
  /** Reads what is missing or stale; `force` reads the numbers again regardless. */
  prefetch: (force?: boolean) => void;
  setTrims: (trims: TrimOriginals) => void;
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
      const s = get();
      if (!usageBusy && (force || isStale(s.at, Date.now(), STORAGE_MAX_AGE_MS))) {
        usageBusy = true;
        api
          .storageUsage()
          .then((usage: StorageUsage) => keep({ usage, at: Date.now() }))
          .catch(() => {
            // An unreadable folder is a missing number, not an error.
          })
          .finally(() => {
            usageBusy = false;
          });
      }
      if (!trimsBusy && (force || isStale(s.at, Date.now(), STORAGE_MAX_AGE_MS) || !s.trims)) {
        trimsBusy = true;
        api
          .trimOriginals()
          .then((trims) => keep({ trims }))
          .catch(() => get().trims ?? keep({ trims: { count: 0, bytes: 0 } }))
          .finally(() => {
            trimsBusy = false;
          });
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
  };
});
