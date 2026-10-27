// Who may reach a user who has "Appear offline" on. An invisible user must
// look offline to relays too, yet the replies to the clips they send (decline,
// cancel, failure) come back through the same relay — so a friend gets through
// only for a while after the invisible user wrote to them.
//
// No imports from the rest of the server, so `node --test` can load this file
// as it is (hub.ts imports cloudflare:workers).

/** How long a friend stays reachable after the invisible user wrote to them. */
export const CONTACT_MS = 600_000;
/** Most friends remembered at once; the newest win. */
export const CONTACT_MAX = 50;

/** Peer id → when the permission ends, in ms since the epoch. */
export type Contacts = Record<string, number>;

/** Notes that the user wrote to `to`: drops what expired, keeps the newest `CONTACT_MAX`. */
export function remember(contacts: Contacts, to: string, now: number): Contacts {
  const live = Object.entries({ ...contacts, [to]: now + CONTACT_MS })
    .filter(([, expiry]) => expiry > now)
    .sort((a, b) => b[1] - a[1])
    .slice(0, CONTACT_MAX);
  return Object.fromEntries(live);
}

/** May a relay from `from` reach the invisible user? */
export function mayReach(contacts: Contacts | undefined, from: string, now: number): boolean {
  const expiry = contacts?.[from];
  return expiry !== undefined && expiry > now;
}

/**
 * Whether a presence change hides the user — "Appear offline" turned on, or
 * the last app gone. That one is published at once, past the presence budget:
 * the throttle must never keep a user visible who asked not to be. It cannot
 * be used to flood: showing up again still spends from the budget.
 */
export function hidesNow(published: object | null, next: object | null): boolean {
  return published !== null && next === null;
}

// --- Budgets and limits -----------------------------------------------------

/** A budget in use: tokens left at `at`. */
export type Bucket = { tokens: number; at: number };
/** A budget's size and how long one token takes to come back. */
export type Rule = { capacity: number; refillMs: number };

/** Relayed messages per user: 30 per 10 s, as before, but shared by all connections. */
export const RELAY: Rule = { capacity: 30, refillMs: 333 };
/** Presence published to friends: ten at once, then one per 30 s. */
export const PRESENCE: Rule = { capacity: 10, refillMs: 30_000 };
/** Hellos per user, so a reconnect loop cannot ask for a snapshot every second. */
export const HELLO: Rule = { capacity: 5, refillMs: 60_000 };
/** Larger raw messages are dropped before they are parsed. */
export const RAW_MAX = 8192;
/** Connections per user; the oldest beyond this are closed. */
export const SOCKET_CAP = 4;

/**
 * Takes one token from a budget; `undefined` is a full one. Tokens come back
 * continuously, never above `capacity`. `nextAt` is when a token is there.
 */
export function spend(
  bucket: Bucket | undefined,
  rule: Rule,
  now: number,
): { bucket: Bucket; ok: boolean; nextAt: number } {
  const before = bucket
    ? Math.min(rule.capacity, bucket.tokens + Math.max(0, now - bucket.at) / rule.refillMs)
    : rule.capacity;
  if (before >= 1) return { bucket: { tokens: before - 1, at: now }, ok: true, nextAt: now };
  return {
    bucket: { tokens: before, at: now },
    ok: false,
    nextAt: now + Math.ceil((1 - before) * rule.refillMs),
  };
}

/** The oldest sockets past `cap`. A socket without `opened` counts as the oldest. */
export function beyondCap<T extends { opened?: number }>(sockets: T[], cap: number): T[] {
  const oldestFirst = [...sockets].sort((a, b) => (a.opened ?? 0) - (b.opened ?? 0));
  return oldestFirst.slice(0, Math.max(0, sockets.length - cap));
}
