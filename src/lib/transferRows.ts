import type { FriendPresence, Transfer } from "./types";

/**
 * How clips on their way show up: in the friend's row on the Friends page,
 * as cards bottom right everywhere else — and how long each stays.
 *
 * - An offer waits in "Pending" (a card elsewhere) until it is answered.
 * - A running transfer shows its bar.
 * - A failure stays, with Retry where that makes sense, until it is dismissed.
 * - Anything else that ended ("has your clip", "said no", "isn't taking clips
 *   right now") stays in the row until
 *   the friend's state changes; its card goes after a few seconds.
 * - Cancelled, or an offer one declined oneself: nothing left to show.
 */

export type TransferKind = "offer" | "active" | "error" | "ended" | "gone";

export function transferKind(t: Transfer): TransferKind {
  if (t.stage === "cancelled") return "gone";
  if (t.direction === "in") {
    if (t.stage === "asking") return "offer";
    if (t.stage === "declined") return "gone";
    if (t.stage === "failed") return "error";
    if (t.stage === "done" || t.stage === "expired") return "ended";
    return "active";
  }
  if (t.stage === "failed" || t.stage === "expired") return "error";
  if (t.stage === "done" || t.stage === "declined") return "ended";
  return "active";
}

/** What "the friend's state" is: whether and what they play, busy, their line. */
export function presenceSig(presence: FriendPresence | null | undefined): string {
  if (!presence) return "offline";
  return `${presence.busy ? "busy" : "on"}|${presence.game ?? ""}|${presence.status ?? ""}`;
}

/** When a transfer ended and how its friend stood at that moment. */
export interface Ended {
  at: number;
  sig: string;
}

/**
 * Notes the transfers that have just ended and forgets the ones gone from the
 * list. Returns `ended` itself when nothing changed.
 */
export function noteEnded(
  transfers: Transfer[],
  ended: Record<string, Ended>,
  presence: Record<string, FriendPresence>,
  now: number,
): Record<string, Ended> {
  const next: Record<string, Ended> = {};
  let changed = false;
  for (const t of transfers) {
    const kind = transferKind(t);
    if (kind === "offer" || kind === "active") continue;
    const known = ended[t.id];
    if (known) next[t.id] = known;
    else {
      next[t.id] = { at: now, sig: presenceSig(presence[t.friendId]) };
      changed = true;
    }
  }
  if (Object.keys(ended).length !== Object.keys(next).length) changed = true;
  return changed ? next : ended;
}

/** Transfers to take off the core's list: the gone ones, and the ended ones
 *  whose friend has moved on since. */
export function toDismiss(
  transfers: Transfer[],
  ended: Record<string, Ended>,
  presence: Record<string, FriendPresence>,
): string[] {
  return transfers
    .filter((t) => {
      const kind = transferKind(t);
      if (kind === "gone") return true;
      if (kind !== "ended") return false;
      const known = ended[t.id];
      return known !== undefined && known.sig !== presenceSig(presence[t.friendId]);
    })
    .map((t) => t.id);
}

/** How long one has to answer an offer — `ANSWER_WITHIN` in `share.rs`. */
export const ANSWER_WITHIN_MS = 60_000;

/**
 * Notes when each offer waiting for an answer was first seen here, and forgets
 * the ones no longer waiting — the countdown for an offer that does not say
 * when it runs out (`offerDeadline`). One shows up at once, so first seen is
 * when it came — or, for one already waiting when the window started, a
 * little later. Returns `seen` itself when nothing changed.
 */
export function noteOffers(transfers: Transfer[], seen: Record<string, number>, now: number): Record<string, number> {
  const next: Record<string, number> = {};
  let changed = false;
  for (const t of transfers) {
    if (transferKind(t) !== "offer") continue;
    next[t.id] = seen[t.id] ?? now;
    if (seen[t.id] === undefined) changed = true;
  }
  if (Object.keys(seen).length !== Object.keys(next).length) changed = true;
  return changed ? next : seen;
}

/**
 * When an offer runs out: when the core says (`expiresAt`) — an offer held
 * until the friends lists were loaded shows up late, with less than a minute
 * left — else a minute after it was first seen here.
 */
export function offerDeadline(expiresAt: number | null | undefined, seenAt: number): number {
  return expiresAt ?? seenAt + ANSWER_WITHIN_MS;
}

/** Whole seconds left to answer an offer that runs out at `deadline`, never below 0. */
export function answerLeft(deadline: number, now: number): number {
  return Math.max(0, Math.ceil((deadline - now) / 1000));
}

/** How long a card for an ended transfer stays on the other pages. */
export const CARD_ENDED_MS = 6_000;

/** Milliseconds a card has left: `Infinity` while it stays, 0 once it is gone. */
export function cardLeft(t: Transfer, ended: Record<string, Ended>, now: number): number {
  const kind = transferKind(t);
  if (kind === "gone") return 0;
  if (kind !== "ended") return Infinity;
  const at = ended[t.id]?.at ?? now;
  return Math.max(0, CARD_ENDED_MS - (now - at));
}

/** Failed cards on screen at once; the rest wait behind "Dismiss all". */
export const FAILED_CARDS_SHOWN = 3;

/**
 * The cards to draw from the ones that stay: everything still running or just
 * ended, and the newest failures up to `cap` — failures never go by
 * themselves, so a bad connection piled them up the window. `failed` is every
 * failure for "Dismiss all", `more` how many of them are not drawn.
 */
export function cardStack(
  shown: Transfer[],
  ended: Record<string, Ended>,
  cap = FAILED_CARDS_SHOWN,
): { cards: Transfer[]; failed: Transfer[]; more: number } {
  const failed = shown.filter((t) => transferKind(t) === "error");
  if (failed.length <= cap) return { cards: shown, failed, more: 0 };
  const at = (t: Transfer) => ended[t.id]?.at ?? Infinity;
  const newest = new Set([...failed].sort((a, b) => at(b) - at(a)).slice(0, cap));
  return {
    cards: shown.filter((t) => transferKind(t) !== "error" || newest.has(t)),
    failed,
    more: failed.length - cap,
  };
}

/** Whether the Friends page shows it itself — an offer in "Pending", the rest
 *  in the row of a friend on the list. */
export function onFriendsPage(t: Transfer, friendIds: ReadonlySet<string>): boolean {
  return transferKind(t) === "offer" || friendIds.has(t.friendId);
}

/** What a row says about the clips going to or coming from that friend. */
export type RowTransfer =
  | { kind: "sending"; ids: string[]; text: string; share: number | null }
  | { kind: "receiving"; ids: string[]; text: string; share: number | null }
  | { kind: "error"; ids: string[]; text: string; retry: Transfer[] }
  | { kind: "done"; ids: string[]; text: string; tone: "ok" | "muted"; at: number; open: string | null };

/** What to call it in a sentence — never "clip" for a screenshot or a recording. */
export function noun(t: Pick<Transfer, "screenshot" | "recording">): string {
  return t.screenshot ? "screenshot" : t.recording ? "recording" : "clip";
}

function plural(ts: Pick<Transfer, "screenshot" | "recording">[]): string {
  const nouns = new Set(ts.map(noun));
  return `${nouns.size === 1 ? [...nouns][0] : "clip"}s`;
}

/** "The connection dropped." → "the connection dropped", to follow a "·". */
export function reason(t: Transfer): string {
  if (t.direction === "out" && t.stage === "expired") return "no answer";
  const text = (t.error ?? "").trim().replace(/\.$/, "");
  if (!text) return "it didn't go through";
  const first = text.split(/\s/, 1)[0];
  return /^[A-Z][a-z]*$/.test(first) ? text[0].toLowerCase() + text.slice(1) : text;
}

/** A clip still waiting its turn to go to the friend (`sendQueue`). */
export type QueuedClip = Pick<Transfer, "name" | "screenshot" | "recording">;

/**
 * The row's transfer line for one friend: running first (with the clips still
 * queued for them counted in), then failures, then what has ended — the newest
 * of those.
 */
export function rowTransfer(
  friendId: string,
  transfers: Transfer[],
  ended: Record<string, Ended>,
  queued: QueuedClip[] = [],
): RowTransfer | null {
  const mine = transfers.filter((t) => t.friendId === friendId);
  const by = (kind: TransferKind) => mine.filter((t) => transferKind(t) === kind);

  const active = by("active");
  const out = active.filter((t) => t.direction === "out");
  const sending = out.length > 0 || queued.length > 0;
  const running = sending ? out : active;
  const waiting = sending ? queued : [];
  if (running.length + waiting.length > 0) {
    const moving = running.filter((t) => t.stage === "moving");
    const size = running.reduce((sum, t) => sum + t.size, 0);
    const moved = running.reduce((sum, t) => sum + t.moved, 0);
    const count = running.length + waiting.length;
    let text: string;
    if (waiting.length > 0 && moving.length === 0) {
      // A chain that hasn't got going: the friend still has to say yes to the first.
      const more = (n: number) => (n > 0 ? ` · ${n} more` : "");
      text =
        running.length === 0
          ? `Getting ${waiting[0].name} ready${more(waiting.length - 1)}`
          : running[0].stage === "asking"
            ? `Waiting for an answer · ${running[0].name}${more(waiting.length)}`
            : `Getting ${running[0].name} ready${more(waiting.length)}`;
    } else if (count > 1) text = `${sending ? "Sending" : "Receiving"} ${count} ${plural([...running, ...waiting])}`;
    else if (!sending) text = `Receiving ${running[0].name}`;
    else if (running[0].stage === "moving") text = `Sending ${running[0].name}`;
    else if (running[0].stage === "asking") text = `Waiting for an answer · ${running[0].name}`;
    else text = `Getting ${running[0].name} ready`;
    return {
      kind: sending ? "sending" : "receiving",
      ids: running.map((t) => t.id),
      text,
      share: moving.length > 0 && size > 0 ? moved / size : null,
    };
  }

  const errors = by("error");
  if (errors.length > 0) {
    const first = errors[0];
    const allOut = errors.every((t) => t.direction === "out");
    let text: string;
    if (errors.length > 1)
      text = allOut ? `Couldn't send ${errors.length} ${plural(errors)}` : `${errors.length} ${plural(errors)} didn't go through`;
    else text = first.direction === "out" ? `Couldn't send ${first.name} · ${reason(first)}` : `${first.name} didn't arrive · ${reason(first)}`;
    return {
      kind: "error",
      ids: errors.map((t) => t.id),
      text,
      retry: errors.filter((t) => t.direction === "out" && t.clipId !== null),
    };
  }

  const over = by("ended").sort((a, b) => (ended[b.id]?.at ?? 0) - (ended[a.id]?.at ?? 0));
  if (over.length > 0) {
    const newest = over[0];
    const unconfirmed = newest.unconfirmed === true;
    const byHand = newest.refusal === "manual";
    const same =
      over.filter(
        (t) =>
          t.direction === newest.direction &&
          t.stage === newest.stage &&
          (t.unconfirmed === true) === unconfirmed &&
          (t.refusal === "manual") === byHand,
      ).length - 1;
    const more = same > 0 ? ` and ${same} more` : "";
    // A no their app gave on its own — or one from an app that does not say
    // whose — is no one's: worded like the banner and the console (`missedLine`).
    if (newest.direction === "out" && newest.stage === "declined" && !byHand)
      return {
        kind: "done",
        ids: over.map((t) => t.id),
        text: `Isn't taking ${noun(newest)}s right now · ${newest.name}${more}`,
        tone: "muted",
        at: ended[newest.id]?.at ?? 0,
        open: null,
      };
    const lead =
      newest.direction === "out"
        ? newest.stage === "done"
          ? unconfirmed
            ? "Sent"
            : "Has your"
          : "Said no to"
        : newest.stage === "done"
          ? "Sent you"
          : "Missed";
    return {
      kind: "done",
      ids: over.map((t) => t.id),
      // Every byte went, but their app never said it arrived.
      text: `${lead} ${newest.name}${more}${unconfirmed ? " · not confirmed" : ""}`,
      tone: newest.stage === "done" && !unconfirmed ? "ok" : "muted",
      at: ended[newest.id]?.at ?? 0,
      open: newest.direction === "in" && newest.stage === "done" ? newest.clipId : null,
    };
  }
  return null;
}
