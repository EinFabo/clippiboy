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
/** How long an upload may still be on its way. A pending share older than
    this broke off: it stops counting and the sweep removes it. The app gives
    up after 20 minutes. */
export const PENDING_MS = 30 * 60 * 1000;
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
  return randomCode(ID_ALPHABET, 10);
}

/** `length` characters drawn from `alphabet` — also the friend codes. Here,
    not in util.ts, because this file is tested under plain Node and imports
    nothing. */
export function randomCode(alphabet: string, length: number): string {
  const bytes = crypto.getRandomValues(new Uint8Array(length));
  return Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join("");
}

/** "in 3 days", "in 5 hours", "in a few minutes" — for the page. */
export function expiresIn(expiresAt: number, now: number): string {
  const left = expiresAt - now;
  // Rounded, not cut: a link made a minute ago has five days, not four.
  const days = Math.round(left / DAY_MS);
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

/** Tags as the page shows them: trimmed, no duplicates, a dozen at most. */
export function cleanTags(raw: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const tag of raw) {
    const text = tag.trim().slice(0, 40);
    const key = text.toLowerCase();
    if (!text || seen.has(key)) continue;
    seen.add(key);
    out.push(text);
    if (out.length === 12) break;
  }
  return out;
}

/** An MP4 starts with its `ftyp` box: four bytes of size, then the name.
    Anything else is not a clip, whatever it calls itself. */
export function looksLikeMp4(head: Uint8Array): boolean {
  return head.length >= 8 && head[4] === 0x66 && head[5] === 0x74 && head[6] === 0x79 && head[7] === 0x70;
}

/** The app's thumbnails are JPEGs, and nothing else goes up as a poster. */
export function looksLikeJpeg(head: Uint8Array): boolean {
  return head.length >= 3 && head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff;
}
