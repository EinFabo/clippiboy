// The rules for share links, without anything of the Worker in them — so they
// can be tested on their own (`npm test`).
//
// Everything lives in R2's free tier: 10 GB stored, downloads free. That is
// why there is a ceiling on the whole bucket, not only per person.

const DAY_MS = 24 * 60 * 60 * 1000;

/** Links one person may make in seven days. A deleted link still counts —
    otherwise delete and upload again would be a way around it. */
export const WEEKLY_LIMIT = 3;
export const WEEK_MS = 7 * DAY_MS;
/** One clip, after the app has shrunk it. Also well under the 100 MB a Worker
    request on the free plan may carry. */
export const MAX_BYTES = 50 * 1024 * 1024;
/** What all live links together may take. A gigabyte under the free 10, for
    uploads in flight and the odd object the hourly sweep has not reached. */
export const TOTAL_BYTES = 9 * 1024 * 1024 * 1024;
export const LIFETIME_MS = 5 * DAY_MS;
/** A preview picture for Discord and the page. */
export const MAX_POSTER_BYTES = 1024 * 1024;

export type Verdict =
  | { ok: true }
  | { ok: false; reason: "too_big" }
  | { ok: false; reason: "quota"; resetsAt: number }
  | { ok: false; reason: "full" };

export interface Quota {
  used: number;
  limit: number;
  /** When the oldest link of the week stops counting, if any does. */
  resetsAt: number | null;
  /** No room for another clip of the largest size right now. */
  full: boolean;
}

/**
 * How much of the week is used. `recent` are the creation times of the
 * person's links in the last seven days, in any order.
 */
export function quota(recent: number[], liveBytes: number): Quota {
  const sorted = [...recent].sort((a, b) => a - b);
  const used = sorted.length;
  // The link whose leaving the window frees a slot: with three of three used,
  // the oldest; with five (an old, looser limit), the third from the end.
  const freeing = used >= WEEKLY_LIMIT ? sorted[used - WEEKLY_LIMIT] : sorted[0];
  return {
    used,
    limit: WEEKLY_LIMIT,
    resetsAt: freeing === undefined ? null : freeing + WEEK_MS,
    full: liveBytes + MAX_BYTES > TOTAL_BYTES,
  };
}

/** May this person upload `bytes` now? The cheap checks first. */
export function canUpload(recent: number[], bytes: number, liveBytes: number): Verdict {
  if (bytes > MAX_BYTES) return { ok: false, reason: "too_big" };
  const state = quota(recent, liveBytes);
  if (state.used >= state.limit) return { ok: false, reason: "quota", resetsAt: state.resetsAt! };
  // Measured against this clip, not the largest: a small one may still fit
  // where `full` already tells the app to stop offering.
  if (liveBytes + bytes > TOTAL_BYTES) return { ok: false, reason: "full" };
  return { ok: true };
}

// Letters and digits without look-alikes — a link gets typed off a screen now
// and then. 31^10 is about 8·10^14: nobody guesses one.
const ID_ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
export const ID_PATTERN = /^[a-hjkmnp-z2-9]{10}$/;

export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(10));
  return Array.from(bytes, (byte) => ID_ALPHABET[byte % ID_ALPHABET.length]).join("");
}

/** "in 3 days", "in 5 hours", "in a few minutes" — for the page. */
export function expiresIn(expiresAt: number, now: number): string {
  const left = expiresAt - now;
  const days = Math.floor(left / DAY_MS);
  if (days >= 2) return `in ${days} days`;
  if (days === 1) return "in 1 day";
  const hours = Math.floor(left / (60 * 60 * 1000));
  if (hours >= 2) return `in ${hours} hours`;
  if (hours === 1) return "in 1 hour";
  return "in a few minutes";
}

/** The part of a Range answer that R2 reports back, as offset and length. */
export function byteRange(
  range: { offset?: number; length?: number; suffix?: number },
  size: number,
): { offset: number; length: number } {
  if (range.suffix !== undefined) {
    const length = Math.min(range.suffix, size);
    return { offset: size - length, length };
  }
  const offset = range.offset ?? 0;
  return { offset, length: range.length ?? size - offset };
}
