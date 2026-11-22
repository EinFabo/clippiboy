import type { Transfer } from "./types";

/**
 * Several clips for one friend, one after another. The core keeps a single
 * unanswered offer per friend (`share.rs`, `unanswered`) — their side would
 * ignore a second one — so the next clip only goes once the friend has said
 * yes to the one before. A no, a failure or no answer ends the chain: the row
 * shows what went wrong, and the rest stays unsent.
 */

/** A clip waiting its turn. */
export interface Queued {
  clipId: string;
  /** For the row while it waits: "Getting Ace on Mirage ready". */
  name: string;
  screenshot: boolean;
  recording: boolean;
  /** A failed transfer this one sends again — dismissed once it is on its way. */
  replaces?: string;
}

/** The clip handed to the core last, and what it became. */
export interface Sent {
  item: Queued;
  /** The transfers there were before it, so the new one can be told apart. */
  known: string[];
  /** Its transfer, once it has turned up in the list. */
  id: string | null;
}

export interface FriendQueue {
  items: Queued[];
  sent: Sent | null;
}

/** By friend id. */
export type SendQueue = Record<string, FriendQueue>;

/** Adds clips to a friend's chain; one already waiting there is not added twice. */
export function enqueue(queue: SendQueue, friendId: string, items: Queued[]): SendQueue {
  const current = queue[friendId] ?? { items: [], sent: null };
  const waiting = new Set(current.items.map((q) => q.clipId));
  const added: Queued[] = [];
  for (const q of items) {
    if (waiting.has(q.clipId)) continue;
    waiting.add(q.clipId);
    added.push(q);
  }
  if (added.length === 0) return queue;
  return { ...queue, [friendId]: { ...current, items: [...current.items, ...added] } };
}

/** The friend's chain ends here — cancelled, or a send that could not start. */
export function dropQueue(queue: SendQueue, friendId: string): SendQueue {
  if (!(friendId in queue)) return queue;
  const next = { ...queue };
  delete next[friendId];
  return next;
}

/** What is still to go to a friend, for the row — the clip just handed over
 *  included until its transfer turns up. */
export function waitingFor(queue: SendQueue, friendId: string): Queued[] {
  const entry = queue[friendId];
  if (!entry) return [];
  return entry.sent && entry.sent.id === null ? [entry.sent.item, ...entry.items] : entry.items;
}

export interface Step {
  /** The queue after this step; the very same object when nothing changed. */
  queue: SendQueue;
  /** Clips to hand to the core now, one per friend at most. */
  send: { friendId: string; item: Queued }[];
  /** Chains that ended on a no or a failure, with what was left unsent. */
  stopped: { friendId: string; left: Queued[] }[];
}

const UNANSWERED: Transfer["stage"][] = ["preparing", "asking"];

/**
 * One look at the transfers: whose last clip has been answered, whose next
 * clip may go, whose chain has ended.
 */
export function step(queue: SendQueue, transfers: Transfer[]): Step {
  let next = queue;
  const send: Step["send"] = [];
  const stopped: Step["stopped"] = [];
  const put = (friendId: string, value: FriendQueue | null) => {
    next = value === null ? dropQueue(next, friendId) : { ...next, [friendId]: value };
  };

  for (const [friendId, entry] of Object.entries(queue)) {
    let { sent } = entry;
    if (sent) {
      let id = sent.id;
      if (id === null) {
        const known = new Set(sent.known);
        const clipId = sent.item.clipId;
        const fresh = transfers.find(
          (t) => t.direction === "out" && t.friendId === friendId && t.clipId === clipId && !known.has(t.id),
        );
        // Handed over, not in the list yet — the core's update is on its way.
        if (!fresh) continue;
        id = fresh.id;
      }
      const t = transfers.find((x) => x.id === id);
      const stage = t?.stage ?? "cancelled";
      if (UNANSWERED.includes(stage)) {
        if (id !== sent.id) put(friendId, { ...entry, sent: { ...sent, id } });
        continue;
      }
      if (stage !== "moving" && stage !== "done") {
        put(friendId, null);
        stopped.push({ friendId, left: entry.items });
        continue;
      }
      sent = null;
    }
    // Something else still waits for their answer — sent from the clip, or
    // from the console.
    const busy = awaitsAnswer(transfers, friendId);
    if (busy || entry.items.length === 0) {
      if (entry.items.length === 0) put(friendId, null);
      else if (sent !== entry.sent) put(friendId, { ...entry, sent });
      continue;
    }
    const [item, ...rest] = entry.items;
    put(friendId, { items: rest, sent: { item, known: transfers.map((t) => t.id), id: null } });
    send.push({ friendId, item });
  }
  return { queue: next, send, stopped };
}

/** An offer to this friend still waits for their answer — the next has to wait. */
export function awaitsAnswer(transfers: Transfer[], friendId: string): boolean {
  return transfers.some((t) => t.direction === "out" && t.friendId === friendId && UNANSWERED.includes(t.stage));
}
