import type { StorageUsage, TrimOriginals } from "./types.ts";

/** What the Storage tab remembers between visits (and between starts). */
export interface StorageCache {
  usage: StorageUsage | null;
  trims: TrimOriginals | null;
  defaultDir: string | null;
  /** Unix ms of the last usage read; `null` while there never was one. */
  at: number | null;
}

export const STORAGE_CACHE_KEY = "cb.storage.cache";

/** Older than this and the numbers are read again when the tab is looked at. */
export const STORAGE_MAX_AGE_MS = 30_000;

/** Controllers: a remembered answer older than this is not shown as current. */
export const CONTROLLERS_MAX_AGE_MS = 120_000;

/** Has `at` (Unix ms, `null` = never) gone stale by `now`? */
export function isStale(at: number | null, now: number, maxAge: number): boolean {
  return at === null || !Number.isFinite(at) || now - at > maxAge || now < at;
}

const USAGE_KEYS = [
  "clipsBytes",
  "originalsBytes",
  "facecamBytes",
  "shotEditsBytes",
  "tracksBytes",
  "thumbsBytes",
  "cacheBytes",
  "toolsBytes",
  "trashBytes",
] as const;

const isBytes = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v) && v >= 0;

/** The stored text back into a cache; anything damaged or foreign is `null`. */
export function parseStorageCache(raw: string | null): StorageCache | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as Record<string, unknown> | null;
    if (!v || typeof v !== "object") return null;
    let usage: StorageUsage | null = null;
    const u = v.usage as Record<string, unknown> | null | undefined;
    if (u && typeof u === "object") {
      if (!USAGE_KEYS.every((k) => isBytes(u[k]))) return null;
      usage = Object.fromEntries(USAGE_KEYS.map((k) => [k, u[k]])) as unknown as StorageUsage;
    }
    let trims: TrimOriginals | null = null;
    const t = v.trims as Record<string, unknown> | null | undefined;
    if (t && typeof t === "object") {
      if (!isBytes(t.count) || !isBytes(t.bytes)) return null;
      trims = { count: t.count, bytes: t.bytes };
    }
    const defaultDir = typeof v.defaultDir === "string" ? v.defaultDir : null;
    const at = isBytes(v.at) ? v.at : null;
    return { usage, trims, defaultDir, at };
  } catch {
    return null;
  }
}

/**
 * Does "Bake all cams" have its card? Decided on the last known size so the
 * card is there from the first frame: unknown keeps it (greyed out) rather
 * than leaving a gap that fills later; a known zero leaves it out.
 */
export function bakeCardShown(lastFacecamBytes: number | null): boolean {
  return lastFacecamBytes === null || lastFacecamBytes > 0;
}

/**
 * Is the saved camera missing from the list? Not before the list has been
 * read once — an empty list then only means "not asked yet".
 */
export function cameraMissing(
  deviceId: string | null,
  ids: readonly string[],
  loaded: boolean,
): boolean {
  return loaded && deviceId !== null && !ids.includes(deviceId);
}

/**
 * Does a read's answer still stand? Not when the data changed (a clip went, a
 * folder moved, "Clear all trims" ran) after the read had started: its
 * numbers may predate the change. `started` and `now` are invalidation counts.
 */
/** An invalidation while a read runs: that read is dropped, so one more is owed after it. */
export function rerunOwed(busy: boolean, already: boolean): boolean {
  return busy || already;
}

export function readStillCurrent(started: number, now: number): boolean {
  return started === now;
}
