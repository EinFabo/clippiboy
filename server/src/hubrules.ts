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
