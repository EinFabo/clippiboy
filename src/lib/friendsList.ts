import type { FriendPending, FriendPresence, FriendUser, Transfer } from "./types";

/**
 * The Friends list's own rules, apart from the page so they can be tested:
 * which group a friend stands in and in what order, what waits in "Pending",
 * and what the nav badge counts.
 */

export type FriendStatus = "playing" | "online" | "busy" | "offline";

export function friendStatus(presence: FriendPresence | null | undefined): FriendStatus {
  if (!presence) return "offline";
  if (presence.busy) return "busy";
  return presence.game ? "playing" : "online";
}

export interface FriendGroups {
  playing: FriendUser[];
  online: FriendUser[];
  offline: FriendUser[];
}

/**
 * "In game", "Online" (Busy included) and "Offline". Favourites first in every
 * group; below that the server's alphabetical order, and offline the ones seen
 * most recently.
 */
export function groupFriends(
  friends: FriendUser[],
  presence: Record<string, FriendPresence>,
  lastSeen: Record<string, number>,
  favorites: string[],
): FriendGroups {
  const fav = (a: FriendUser, b: FriendUser) => Number(favorites.includes(b.id)) - Number(favorites.includes(a.id));
  const sorted = [...friends].sort(fav);
  return {
    playing: sorted.filter((f) => presence[f.id]?.game),
    online: sorted.filter((f) => presence[f.id] && !presence[f.id].game),
    offline: sorted
      .filter((f) => !presence[f.id])
      .sort((a, b) => fav(a, b) || (lastSeen[b.id] ?? 0) - (lastSeen[a.id] ?? 0)),
  };
}

/** One row in "Pending". */
export type PendingRow =
  | { kind: "request"; request: FriendPending }
  | { kind: "offer"; transfer: Transfer }
  | { kind: "sent"; request: FriendPending };

/** Clips someone wants to send that wait for a yes or no. */
export function clipOffers(transfers: Transfer[]): Transfer[] {
  return transfers.filter((t) => t.direction === "in" && t.stage === "asking");
}

/**
 * Requests to answer (newest first), then clips offered, then one's own
 * requests still open — those last, as nothing is to be done about them.
 */
export function pendingRows(
  incoming: FriendPending[],
  outgoing: FriendPending[],
  transfers: Transfer[],
): PendingRow[] {
  const newest = (a: FriendPending, b: FriendPending) => b.since - a.since;
  return [
    ...[...incoming].sort(newest).map((request) => ({ kind: "request" as const, request })),
    ...clipOffers(transfers).map((transfer) => ({ kind: "offer" as const, transfer })),
    ...[...outgoing].sort(newest).map((request) => ({ kind: "sent" as const, request })),
  ];
}

/** The number on Friends in the nav: only what waits for one's answer. */
export function pendingCount(incoming: FriendPending[], transfers: Transfer[]): number {
  return incoming.length + clipOffers(transfers).length;
}

/** "just now", "5 min ago", "2 h ago", "yesterday", "3 days ago". */
export function ago(at: number, now: number): string {
  const minutes = Math.max(0, Math.floor((now - at) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

/** The rows a group shows: all while open; folded, only those with a transfer
 *  line, which would otherwise be on no card and in no row. */
export function foldedRows<T extends { transfer: unknown }>(rows: T[], open: boolean): T[] {
  return open ? rows : rows.filter((row) => row.transfer !== null);
}
