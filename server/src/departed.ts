// What outlives a deleted account: a keyed one-way hash of its Discord id, the
// times of its links that still count against the week, and a suspension if
// one stood. Signing in again with the same Discord account picks them up —
// otherwise deleting the account and coming back would start the weekly link
// limit over and drop the suspension. Kept for a week; a suspended account's
// row is kept for as long as the suspension stands (`departedKept`), since a
// suspended account may be deleted too and its suspension must not run out
// with the week — its link times still go after the week (the hourly cron).
//
// Imports nothing, so it can be tested under plain Node (`npm test`).

const DAY_MS = 24 * 60 * 60 * 1000;

/** How long a row is kept after the account went, unless it holds a suspension. */
export const DEPARTED_KEEP_MS = 7 * DAY_MS;

/** Link times kept per row, newest first — more than the weekly limit will
    ever be, so a returning account is held exactly where it left off. */
export const KEPT_LINKS = 10;

/**
 * HMAC-SHA256 of the Discord id under the secret `DEPARTED_PEPPER`, as
 * base64url. Without the pepper a Discord id — which is no secret — could be
 * hashed by anyone and matched against the table. Null when the secret is not
 * set: then nothing is kept.
 */
export async function fingerprint(pepper: string | undefined, discordId: string): Promise<string | null> {
  if (!pepper) return null;
  const encoder = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", encoder.encode(pepper), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, encoder.encode(discordId)));
  let text = "";
  for (const byte of mac) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Rows that left before this are done with — the hourly cron deletes them,
    except those that hold a suspension (`departedKept`). */
export function departedCutoff(now: number): number {
  return now - DEPARTED_KEEP_MS;
}

/** Whether the hourly cron keeps a row: within its week, or holding a
    suspension. The cron's `DELETE FROM departed` is the same rule in SQL. */
export function departedKept(row: Pick<DepartedRow, "banned_at" | "left_at">, now: number): boolean {
  return row.banned_at != null || row.left_at > departedCutoff(now);
}

export interface DepartedRow {
  link_times: string | null;
  banned_at: number | null;
  left_at: number;
}

export interface CarryOver {
  /** Creation times of links that still count against the week, oldest first. */
  linkTimes: number[];
  bannedAt: number | null;
}

/**
 * What a new account for the same Discord id takes over from its departed
 * row: the suspension, however old the row, and the link times still inside
 * the week — none once the row is a week old, even if the cron has not
 * removed it yet.
 */
export function carryOver(row: DepartedRow | null, now: number): CarryOver | null {
  if (!row) return null;
  const bannedAt = row.banned_at ?? null;
  if (row.left_at <= departedCutoff(now)) return bannedAt === null ? null : { linkTimes: [], bannedAt };
  let parsed: unknown = [];
  try {
    parsed = JSON.parse(row.link_times ?? "[]");
  } catch {
    // An unreadable list carries no links; the suspension still holds.
  }
  const linkTimes = (Array.isArray(parsed) ? parsed : [])
    .filter((t): t is number => typeof t === "number" && Number.isFinite(t) && t > now - DEPARTED_KEEP_MS && t <= now)
    .sort((a, b) => a - b)
    .slice(-KEPT_LINKS);
  if (linkTimes.length === 0 && bannedAt === null) return null;
  return { linkTimes, bannedAt };
}
