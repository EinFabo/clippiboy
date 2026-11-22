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
  /** Stopped while `sent` was still on its way to the core: its transfer is
   *  cancelled as soon as it turns up. */
  cancelSent?: boolean;
}

/** By friend id. */
export type SendQueue = Record<string, FriendQueue>;

const UNFINISHED: Transfer["stage"][] = ["preparing", "asking", "moving"];

/**
 * Adds clips to a friend's chain. Nothing goes twice: not a clip already
 * waiting, just handed over or still on its way to them, and not a second
 * retry of the same failure — a double click adds nothing.
 */
export function enqueue(queue: SendQueue, friendId: string, items: Queued[], transfers: Transfer[] = []): SendQueue {
  const current = queue[friendId] ?? { items: [], sent: null };
  const inChain = current.sent && !current.cancelSent ? [current.sent.item, ...current.items] : current.items;
  const clips = new Set(inChain.map((q) => q.clipId));
  const retries = new Set(inChain.flatMap((q) => (q.replaces ? [q.replaces] : [])));
  for (const t of transfers)
    if (t.direction === "out" && t.friendId === friendId && t.clipId && UNFINISHED.includes(t.stage)) clips.add(t.clipId);
  const added: Queued[] = [];
  for (const q of items) {
    if (clips.has(q.clipId) || (q.replaces && retries.has(q.replaces))) continue;
    clips.add(q.clipId);
    if (q.replaces) retries.add(q.replaces);
    added.push(q);
  }
  if (added.length === 0) return queue;
  return { ...queue, [friendId]: { ...current, items: [...current.items, ...added] } };
}

/**
 * The row's X: nothing more goes. A clip already handed to the core but not
 * yet in the list is remembered, so its transfer is cancelled when it shows.
 */
export function stopQueue(queue: SendQueue, friendId: string): SendQueue {
  const entry = queue[friendId];
  if (entry?.sent && entry.sent.id === null) return { ...queue, [friendId]: { items: [], sent: entry.sent, cancelSent: true } };
  return dropQueue(queue, friendId);
}

/** Whether a failed transfer is already queued to go again — its Retry is done. */
export function retrying(queue: SendQueue, transferId: string): boolean {
  return Object.values(queue).some(
    (entry) =>
      entry.items.some((q) => q.replaces === transferId) || (!entry.cancelSent && entry.sent?.item.replaces === transferId),
  );
}

/** The friend's chain ends here — a send that could not start. */
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
  return entry.sent && entry.sent.id === null && !entry.cancelSent ? [entry.sent.item, ...entry.items] : entry.items;
}

export interface Step {
  /** The queue after this step; the very same object when nothing changed. */
  queue: SendQueue;
  /** Clips to hand to the core now, one per friend at most. */
  send: { friendId: string; item: Queued }[];
  /** Chains that ended on a no or a failure, with what was left unsent. */
  stopped: { friendId: string; left: Queued[] }[];
  /** Transfers to call off — handed over before the row's X. */
  cancel: string[];
  /** Clips no longer in the library, left out on the way. */
  skipped: { friendId: string; item: Queued }[];
}

const UNANSWERED: Transfer["stage"][] = ["preparing", "asking"];

/**
 * One look at the transfers: whose last clip has been answered, whose next
 * clip may go, whose chain has ended. `exists` says whether a clip is still in
 * the library — a deleted one is skipped, the chain goes on.
 */
export function step(queue: SendQueue, transfers: Transfer[], exists: (clipId: string) => boolean = () => true): Step {
  let next = queue;
  const send: Step["send"] = [];
  const stopped: Step["stopped"] = [];
  const cancel: string[] = [];
  const skipped: Step["skipped"] = [];
  const put = (friendId: string, value: FriendQueue | null) => {
    next = value === null ? dropQueue(next, friendId) : { ...next, [friendId]: value };
  };

  for (const [friendId, entry] of Object.entries(queue)) {
    let { sent } = entry;
    if (sent) {
      let id = sent.id;
      if (id === null) {
        const fresh = appeared(transfers, friendId, sent.item.clipId, sent.known);
        // Handed over, not in the list yet — the core's update is on its way.
        if (!fresh) continue;
        id = fresh.id;
        if (entry.cancelSent) {
          cancel.push(id);
          if (entry.items.length === 0) {
            put(friendId, null);
            continue;
          }
          // Called off — the core still lists it as waiting for a moment, so
          // the next clip waits for that below.
          sent = null;
        }
      }
      const t = transfers.find((x) => x.id === id);
      const stage = sent === null ? "done" : (t?.stage ?? "cancelled");
      if (sent && UNANSWERED.includes(stage)) {
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
    let items = entry.items;
    while (!busy && items.length > 0 && !exists(items[0].clipId)) {
      skipped.push({ friendId, item: items[0] });
      items = items.slice(1);
    }
    if (busy || items.length === 0) {
      if (items.length === 0) put(friendId, null);
      else if (sent !== entry.sent) put(friendId, { items, sent });
      continue;
    }
    const [item, ...rest] = items;
    put(friendId, { items: rest, sent: { item, known: transfers.map((t) => t.id), id: null } });
    send.push({ friendId, item });
  }
  return { queue: next, send, stopped, cancel, skipped };
}

/** An offer to this friend still waits for their answer — the next has to wait. */
export function awaitsAnswer(transfers: Transfer[], friendId: string): boolean {
  return transfers.some((t) => t.direction === "out" && t.friendId === friendId && UNANSWERED.includes(t.stage));
}

/**
 * The transfer a send made, once it is in the list: going out to that friend,
 * of that clip, and not one there was before. A send that fails with none
 * never started — only then is its reason not already in the row.
 */
export function appeared(
  transfers: Transfer[],
  friendId: string,
  clipId: string,
  known: Iterable<string>,
): Transfer | undefined {
  const before = new Set(known);
  return transfers.find((t) => t.direction === "out" && t.friendId === friendId && t.clipId === clipId && !before.has(t.id));
}
