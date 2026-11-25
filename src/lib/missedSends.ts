import type { Transfer } from "./types";

/**
 * One's own sends that did not get there — a no, no answer, a failure after
 * the click — as the console lists them, each with Retry, for a while after
 * they ended. The banner that told of it (`share.rs`) says where the Retry is.
 */

/** How long the console keeps one after it ended. */
export const MISSED_SHOWN_MS = 10 * 60_000;
/** How many it lists at most, the newest first. */
export const MISSED_MAX = 3;

export function isMissed(t: Transfer): boolean {
  return t.direction === "out" && (t.stage === "declined" || t.stage === "expired" || t.stage === "failed");
}

function noun(t: Pick<Transfer, "screenshot" | "recording">): string {
  return t.screenshot ? "screenshot" : t.recording ? "recording" : "clip";
}

/** What happened, with `who` as the friend is to be named. A no their app
 *  gave on its own — or one from an app that does not say whose — is no one's. */
export function missedLine(t: Transfer, who: string): string {
  if (t.stage === "declined") {
    return t.refusal === "manual" ? `${who} said no` : `${who} isn't taking ${noun(t)}s right now`;
  }
  if (t.stage === "expired") return `${who} didn't answer`;
  return `${who} didn't get your ${noun(t)}`;
}

/** The line under it: the clip, and for a failure why. */
export function missedDetail(t: Transfer): string {
  const why = t.stage === "failed" ? (t.error ?? "").trim().replace(/\.$/, "") : "";
  return why ? `${t.name} · ${why}` : t.name;
}

/** Notes when each missed send was first seen and forgets those gone from the
 *  list. Returns `seen` itself when nothing changed. */
export function noteMissed(
  transfers: Transfer[],
  seen: Record<string, number>,
  now: number,
): Record<string, number> {
  const next: Record<string, number> = {};
  let changed = false;
  for (const t of transfers) {
    if (!isMissed(t)) continue;
    const at = seen[t.id];
    if (at === undefined) changed = true;
    next[t.id] = at ?? now;
  }
  if (Object.keys(next).length !== Object.keys(seen).length) changed = true;
  return changed ? next : seen;
}

/** The ones to list now: seen within {@link MISSED_SHOWN_MS}, newest first. */
export function shownMissed(transfers: Transfer[], seen: Record<string, number>, now: number): Transfer[] {
  return transfers
    .filter((t) => isMissed(t) && seen[t.id] !== undefined && now - seen[t.id] < MISSED_SHOWN_MS)
    .sort((a, b) => seen[b.id] - seen[a.id])
    .slice(0, MISSED_MAX);
}
