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
    this broke off: the sweep gives it up like a failed upload (`givenUp`). The
    app gives up after 20 minutes. */
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

/** Where a person's week begins: seven days back, unless their limit was
    reset since — links from before a reset no longer count. */
export function weekStart(now: number, resetAt: number): number {
  return Math.max(now - WEEK_MS, resetAt);
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

export type ByteRange =
  /** No Range, or one we do not serve (several ranges, bad syntax): the whole file, 200. */
  | { kind: "whole" }
  /** Starts past the end: 416. */
  | { kind: "unsatisfiable" }
  | { kind: "part"; offset: number; length: number };

/**
 * A Range header against a file of `size` bytes. One range only, as players
 * ask for it; anything else is ignored and gets the whole file, which a server
 * may always do. Only what is checked here reaches R2 — passed on raw, a bad
 * or unsatisfiable range came back as a 206 or a 500.
 */
export function parseRange(header: string | null, size: number): ByteRange {
  const match = header?.trim().match(/^bytes=(\d*)-(\d*)$/i);
  if (!match) return { kind: "whole" };
  const [, first, last] = match;
  if (first === "") {
    // The last n bytes.
    if (last === "") return { kind: "whole" };
    const suffix = Number(last);
    if (suffix === 0 || size === 0) return { kind: "unsatisfiable" };
    const length = Math.min(suffix, size);
    return { kind: "part", offset: size - length, length };
  }
  const offset = Number(first);
  const end = last === "" ? Infinity : Number(last);
  // "9-5" is no range at all; "5000-" on a smaller file one that cannot be met.
  if (end < offset) return { kind: "whole" };
  if (offset >= size) return { kind: "unsatisfiable" };
  return { kind: "part", offset, length: Math.min(end, size - 1) - offset + 1 };
}

/** `items` in runs of at most `size` — R2 deletes at most a thousand keys at once. */
export function chunks<T>(items: readonly T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}

/**
 * Is a share row in the running for its owner's week (and, while not deleted,
 * the bucket)? Finished links are, deleted ones too, and an upload still in
 * time to finish. `COUNTS` in links.ts is the same rule in SQL. Of the failed
 * uploads among them, `weekTimes` then lets a few go free.
 */
export function countsAgainstWeek(row: { pending: number; created_at: number }, now: number): boolean {
  return row.pending === 0 || row.created_at > now - PENDING_MS;
}

/** Uploads a week that may fail without costing a link. */
export const FREE_FAILS = 5;

export interface ShareState {
  pending: number;
  failed: number;
  bytes: number;
  created_at: number;
  deleted_at: number | null;
}

/**
 * An upload that will not finish, as `GAVE_UP` in links.ts leaves it: done,
 * empty, deleted and failed — out of the bucket, but in the week like any
 * failed upload. Also what the hourly sweep does to one that broke off without
 * its own error path running (the request cut off, say). It used to delete
 * those, and an upload abandoned on purpose cost nothing: three 50 MB of the
 * bucket held for half an hour, again and again.
 */
export function givenUp<T extends ShareState>(row: T, now: number): T {
  if (row.pending !== 1) return row;
  return { ...row, pending: 0, failed: 1, bytes: 0, deleted_at: now };
}

/**
 * Why a share may not take its poster now, or null when it may. A deleted or
 * failed one is gone; one still uploading is no link yet, and a poster put
 * then was left behind in R2 when the upload failed.
 */
export function posterRefusal(row: { pending: number; deleted_at: number | null }): "gone" | "uploading" | null {
  if (row.deleted_at !== null) return "gone";
  if (row.pending === 1) return "uploading";
  return null;
}

/**
 * The creation times that count against the week, from the rows in it:
 * every link, and of the failed uploads (`failed` 1 — kept as a deleted row,
 * not removed) all but the newest FREE_FAILS. A dropped line or a cancelled
 * upload now and then costs nothing; failing on purpose — a byte that is no
 * MP4, over and over, each try a row written — runs into the limit soon.
 *
 * The oldest failures are the ones that count: when the oldest leaves the
 * week, one fewer counts, so `quota` frees a slot at the right moment.
 * `RECENT` in links.ts and `departStatement` in db.ts say the same in SQL.
 */
export function weekTimes(rows: { created_at: number; failed: number }[]): number[] {
  const links = rows.filter((row) => row.failed !== 1).map((row) => row.created_at);
  const failed = rows
    .filter((row) => row.failed === 1)
    .map((row) => row.created_at)
    .sort((a, b) => a - b);
  return [...links, ...failed.slice(0, Math.max(0, failed.length - FREE_FAILS))];
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

/** Why someone may report a clip — the same list the page offers. */
export const REPORT_REASONS = ["nsfw", "violence", "hate", "spam", "other"] as const;
export type ReportReason = (typeof REPORT_REASONS)[number];
/** Addresses whose open reports one clip may collect; more say nothing new.
    Counted per address (an IPv6 address per /64), one open report each: as a
    plain count, one address filled it in four minutes and every report after
    that was dropped. */
export const MAX_OPEN_REPORTS = 20;
/** Reports go after this, handled or not — a clip lives five days anyway. */
export const REPORT_KEEP_MS = 30 * DAY_MS;
/** How long a reported clip is held at most once its link is gone. */
export const HELD_MS = 30 * DAY_MS;

/**
 * Whether a report from `reporter` (the fingerprint of an address, see
 * `reportShare`) is taken, given the clip's open reports. `reportShare` in
 * links.ts is the same rule in SQL. Reports from before the fingerprint have
 * none and count for nobody.
 */
export function takesReport(open: { reporter: string | null }[], reporter: string): boolean {
  const reporters = new Set(open.map((row) => row.reporter).filter((r): r is string => r !== null));
  return !reporters.has(reporter) && reporters.size < MAX_OPEN_REPORTS;
}

/**
 * A clip whose link went (deleted, run out, its account deleted or
 * suspended) while a report on it was open is held in R2 for the review,
 * reachable by nobody — the row that made it a link is gone. It goes once no
 * report on it is open any more, and after `HELD_MS` in any case. The hourly
 * sweep's "held" step is the same rule in SQL.
 */
export function heldClipGoes(heldAt: number, openReports: number, now: number): boolean {
  return openReports === 0 || heldAt <= now - HELD_MS;
}

/** A report as the page sent it, or null if it is not one. */
export function cleanReport(raw: unknown): { reason: ReportReason; note: string | null } | null {
  if (!raw || typeof raw !== "object") return null;
  const { reason, note } = raw as { reason?: unknown; note?: unknown };
  if (typeof reason !== "string" || !(REPORT_REASONS as readonly string[]).includes(reason)) return null;
  const text = typeof note === "string" ? note.trim().slice(0, 300) : "";
  return { reason: reason as ReportReason, note: text || null };
}

/**
 * Follows an MP4's top-level boxes as the upload streams past: `ftyp` first,
 * every box with a sane length and a readable name, a `moov` somewhere, and
 * the last box ending exactly where the file does. A file with a few bytes of
 * `ftyp` glued in front, or anything hung on behind, fails.
 *
 * What it cannot rule out is a payload inside a box — only re-encoding could.
 * The page and the file are served as video with `nosniff`, which is what
 * keeps such a file a (broken) video.
 */
export class Mp4Boxes {
  private header: number[] = [];
  /** Bytes still to pass in the current box's body. */
  private skip = 0;
  /** The current box runs to the end of the file (size 0). */
  private toEnd = false;
  private boxes = 0;
  private moov = false;
  private bad = false;

  /** False as soon as the file cannot be an MP4. */
  push(chunk: Uint8Array): boolean {
    let i = 0;
    while (!this.bad && i < chunk.length) {
      if (this.toEnd) return true;
      if (this.skip > 0) {
        const step = Math.min(this.skip, chunk.length - i);
        this.skip -= step;
        i += step;
        continue;
      }
      this.header.push(chunk[i++]);
      this.readHeader();
    }
    return !this.bad;
  }

  /** At the end of the upload: was it a whole MP4? */
  end(): boolean {
    return !this.bad && this.header.length === 0 && (this.skip === 0 || this.toEnd) && this.moov;
  }

  private readHeader(): void {
    const h = this.header;
    if (h.length < 8) return;
    const size32 = ((h[0] << 24) >>> 0) + (h[1] << 16) + (h[2] << 8) + h[3];
    if (size32 === 1 && h.length < 16) return;
    const type = String.fromCharCode(h[4], h[5], h[6], h[7]);
    const size = size32 === 1 ? (h[8] * 2 ** 24 + (h[9] << 16) + (h[10] << 8) + h[11]) * 2 ** 32 + ((h[12] << 24) >>> 0) + (h[13] << 16) + (h[14] << 8) + h[15] : size32;
    const first = this.boxes === 0;
    this.boxes++;
    if (!/^[\x20-\x7e]{4}$/.test(type) || (first && (type !== "ftyp" || size < 16 || size > 1024))) {
      this.bad = true;
      return;
    }
    if (type === "moov") this.moov = true;
    if (size === 0) {
      this.toEnd = true;
    } else if (size < h.length || size > MAX_BYTES) {
      this.bad = true;
    } else {
      this.skip = size - h.length;
    }
    this.header = [];
  }
}

/** The app's thumbnails are JPEGs, and nothing else goes up as a poster. */
export function looksLikeJpeg(head: Uint8Array): boolean {
  return head.length >= 3 && head[0] === 0xff && head[1] === 0xd8 && head[2] === 0xff;
}

/** The upload's `friend` parameter: only an explicit "0" turns the button off — older apps send nothing. */
export function friendOffer(param: string | null): 0 | 1 {
  return param === "0" ? 0 : 1;
}

/**
 * "Add <name> as friend" on a share page: the name is shown, the uploader
 * offered it on this link, and takes requests at all.
 */
export function offersFriend(share: { show_name: number; offer_friend: number }, allowRequests: number): boolean {
  return share.show_name === 1 && share.offer_friend === 1 && allowRequests === 1;
}
