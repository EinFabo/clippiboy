// What a friend request does, without the database — one rule for the Add
// field in the app and for the request an invite sends at sign-in.
//
// Imports nothing, so it can be tested under plain Node (`npm test`).

export const NOBODY = "Nobody with that name or code uses ClippiBoy.";

/** Requests one account may have waiting for an answer at once. */
export const MAX_PENDING_OUT = 20;

/**
 * How long a pair rests once a request between them was withdrawn or
 * declined: neither may ask the other again before. Asking and taking it back
 * in a loop sent the other side a new request every few seconds, and woke
 * both sides' hubs each time.
 */
export const REQUEST_COOLDOWN_MS = 10 * 60 * 1000;

export type PairStatus = "pending" | "accepted";

export interface RequestFacts {
  me: string;
  target: { id: string; allowRequests: boolean };
  /** They blocked me. */
  blockedByTarget: boolean;
  /** I blocked them. */
  blockedByMe: boolean;
  /** The pair's row, if there is one. */
  existing: { status: "pending" | "accepted"; requester: string } | null;
  /** Found by friend code (or an invite) rather than by Discord name. */
  handedOut: boolean;
  /** My requests still waiting for an answer, to anyone. */
  pendingOut: number;
  /** When the pair's rest after a withdrawn or declined request ends, if it has one. */
  cooldownUntil: number | null;
  now: number;
}

export type RequestOutcome =
  | { kind: "send" }
  /** They asked first — asking back is saying yes. */
  | { kind: "accept" }
  | { kind: "refuse"; status: number; message: string };

export function requestRule(f: RequestFacts): RequestOutcome {
  // Someone who blocked you looks like someone who does not exist.
  if (f.blockedByTarget) return { kind: "refuse", status: 404, message: NOBODY };
  if (f.target.id === f.me) return { kind: "refuse", status: 400, message: "That's you." };
  if (f.blockedByMe) return { kind: "refuse", status: 409, message: "You blocked them — unblock them first." };
  if (f.existing?.status === "accepted") return { kind: "refuse", status: 409, message: "You are already friends." };
  if (f.existing?.requester === f.me) return { kind: "refuse", status: 409, message: "Request already sent." };
  if (f.existing) return { kind: "accept" };
  if (!f.target.allowRequests) {
    // Found by name, someone who takes no requests looks like nobody too; a
    // friend code was handed out on purpose, so it may say no. A pair that
    // already has a row never gets here, so accepting back still works.
    return f.handedOut
      ? { kind: "refuse", status: 403, message: "They don't take friend requests right now." }
      : { kind: "refuse", status: 404, message: NOBODY };
  }
  // After the checks above, so it tells nothing they would not: someone who
  // is nobody by name stays nobody. A pair that rests has no row, so saying
  // yes is never held up by it.
  if (f.cooldownUntil !== null && f.cooldownUntil > f.now) {
    return { kind: "refuse", status: 429, message: "Wait a few minutes before asking them again." };
  }
  // Last, so it tells nothing a sent request would not: past the cap only a
  // new request is refused, never saying yes to one that came in.
  if (f.pendingOut >= MAX_PENDING_OUT) {
    return {
      kind: "refuse",
      status: 429,
      message: "You have too many requests waiting — withdraw some or wait for answers.",
    };
  }
  return { kind: "send" };
}

/**
 * Does removing this row rest the pair (`REQUEST_COOLDOWN_MS`)? A request
 * withdrawn or declined does, whichever side removed it, by hand or with a
 * block; an ended friendship does not — asking again needs the other's yes —
 * nor a removal that found nothing. `removeFriend` and `blockUser` in
 * index.ts say the same in SQL (`status = 'pending'`).
 */
export function coolsDown(removed: { status: PairStatus } | null): boolean {
  return removed?.status === "pending";
}

/**
 * What a change to a pair's row tells both sides' apps, as `before` and
 * `after` the change (null: no row): their lists, or their lists and a fresh
 * snapshot of their friends' presence — the latter only when who is a friend
 * changed, since a snapshot asks the hub of every friend. A request sent,
 * withdrawn or declined changes nobody's presence. Nothing when nothing changed.
 */
export function noticeFor(before: PairStatus | null, after: PairStatus | null): "lists" | "presence" | null {
  if (before === after) return null;
  return before === "accepted" || after === "accepted" ? "presence" : "lists";
}
