// What a friend request does, without the database — one rule for the Add
// field in the app and for the request an invite sends at sign-in.
//
// Imports nothing, so it can be tested under plain Node (`npm test`).

export const NOBODY = "Nobody with that name or code uses ClippiBoy.";

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
  return { kind: "send" };
}
