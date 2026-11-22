import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import { clipName } from "./format";
import { useFriends } from "./friends";
import { toast } from "./toastBus";
import { noteEnded, noun, toDismiss, type Ended } from "./transferRows";
import { dropQueue, enqueue, step, type Queued, type SendQueue } from "./sendQueue";
import type { Clip, FriendUser, Transfer } from "./types";

/**
 * Clips going to and coming from friends. The core (`share.rs`) does the
 * moving; this mirrors its list and forwards what the user decides.
 */
export const shareApi = {
  send: (clipId: string, friendId: string) => invoke<void>("share_send", { clipId, friendId }),
  accept: (id: string) => invoke<void>("share_accept", { id }),
  decline: (id: string) => invoke<void>("share_decline", { id }),
  cancel: (id: string) => invoke<void>("share_cancel", { id }),
  /** Off the list once it is over. */
  dismiss: (id: string) => invoke<void>("share_dismiss", { id }),
  clear: () => invoke<void>("share_clear"),
};

interface ShareState {
  transfers: Transfer[];
  /** The clip whose "Send to a friend" picker is open. */
  picking: Clip | null;
  /** Opens the picker for a clip, or closes it. */
  pick: (clip: Clip | null) => void;
  /** The friend whose "Send a clip" picker is open — the same, the other way round. */
  choosingFor: FriendUser | null;
  chooseFor: (friend: FriendUser | null) => void;
}

export const useShare = create<ShareState>((set) => ({
  transfers: [],
  picking: null,
  pick: (clip) => set({ picking: clip }),
  choosingFor: null,
  chooseFor: (friend) => set({ choosingFor: friend }),
}));

if (inTauri) {
  void listen<Transfer[]>("share-state", (event) => useShare.setState({ transfers: event.payload }));
  void invoke<Transfer[]>("share_state").then((transfers) => useShare.setState({ transfers }));
}

export function isOver(transfer: Transfer): boolean {
  return ["done", "declined", "expired", "cancelled", "failed"].includes(transfer.stage);
}

/** "48 MB", "3.2 MB". */
export function megabytes(bytes: number): string {
  const mb = bytes / 1_048_576;
  return mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
}

/** When each finished transfer ended and how its friend stood then — what
 *  decides how long it stays in the row and on its card. */
export const useTransferMemory = create<{ ended: Record<string, Ended> }>(() => ({ ended: {} }));

/** Already asked to go, so a burst of updates asks once. */
const dismissing = new Set<string>();

/**
 * Keeps the core's list tidy for the main window: notes when transfers end,
 * takes the cancelled ones off at once and the ended ones once their friend
 * has moved on. Failures stay until they are dismissed. Returns the stop.
 */
export function watchTransfers(): () => void {
  const check = () => {
    runQueue();
    const { transfers } = useShare.getState();
    const { presence } = useFriends.getState();
    const before = useTransferMemory.getState().ended;
    const ended = noteEnded(transfers, before, presence, Date.now());
    if (ended !== before) useTransferMemory.setState({ ended });
    for (const id of toDismiss(transfers, ended, presence)) {
      if (dismissing.has(id)) continue;
      dismissing.add(id);
      shareApi
        .dismiss(id)
        .catch(() => {})
        .finally(() => dismissing.delete(id));
    }
  };
  check();
  const stopShare = useShare.subscribe(check);
  const stopQueue = useSendQueue.subscribe((next, prev) => {
    if (next.queue !== prev.queue) runQueue();
  });
  const stopFriends = useFriends.subscribe((next, prev) => {
    if (next.presence !== prev.presence) check();
  });
  return () => {
    stopShare();
    stopQueue();
    stopFriends();
  };
}

/** Clips waiting their turn, by friend — see `sendQueue`. Run by the main
 *  window (`watchTransfers`), where the pickers are. */
export const useSendQueue = create<{ queue: SendQueue }>(() => ({ queue: {} }));

/** Sends clips to a friend one after another, each once the one before is answered. */
export function sendClips(friendId: string, clips: Clip[]): void {
  const items = clips.map((clip) => ({
    clipId: clip.id,
    name: clipName(clip),
    screenshot: clip.screenshot,
    recording: clip.recording,
  }));
  useSendQueue.setState((s) => ({ queue: enqueue(s.queue, friendId, items) }));
}

/** Nothing more goes to this friend — the row's X. */
export function stopSending(friendId: string): void {
  useSendQueue.setState((s) => ({ queue: dropQueue(s.queue, friendId) }));
}

function friendName(friendId: string): string {
  return useFriends.getState().lists.friends.find((f) => f.id === friendId)?.displayName ?? "your friend";
}

/** "2 more clips weren't sent to Luca." */
function leftUnsent(left: Queued[], friendId: string): string {
  const nouns = new Set(left.map(noun));
  const what = nouns.size === 1 ? [...nouns][0] : "clip";
  return left.length === 1
    ? `1 more ${what} wasn't sent to ${friendName(friendId)}.`
    : `${left.length} more ${what}s weren't sent to ${friendName(friendId)}.`;
}

function runQueue(): void {
  const { queue } = useSendQueue.getState();
  const next = step(queue, useShare.getState().transfers);
  if (next.queue !== queue) useSendQueue.setState({ queue: next.queue });
  for (const { friendId, left } of next.stopped) if (left.length > 0) toast("error", leftUnsent(left, friendId));
  for (const { friendId, item } of next.send) {
    shareApi
      .send(item.clipId, friendId)
      .then(() => {
        if (item.replaces) void shareApi.dismiss(item.replaces).catch(() => {});
      })
      .catch((err) => {
        // It could not start: the chain ends here, and says so.
        const left = useSendQueue.getState().queue[friendId]?.items ?? [];
        stopSending(friendId);
        const why = typeof err === "string" ? err : `Couldn't send it to ${friendName(friendId)}.`;
        toast("error", left.length > 0 ? `${why} ${leftUnsent(left, friendId)}` : why);
      });
  }
}

/** Sends the clips of failed transfers again, through the same chain; each
 *  failure goes once its new transfer is on its way, and stays if that cannot
 *  start either. */
export function retryTransfers(failed: Transfer[]): void {
  const byFriend = new Map<string, Queued[]>();
  for (const t of failed) {
    if (t.direction !== "out" || !t.clipId) continue;
    const item = { clipId: t.clipId, name: t.name, screenshot: t.screenshot, recording: t.recording, replaces: t.id };
    byFriend.set(t.friendId, [...(byFriend.get(t.friendId) ?? []), item]);
  }
  for (const [friendId, items] of byFriend) useSendQueue.setState((s) => ({ queue: enqueue(s.queue, friendId, items) }));
}

/** What App listens for to open a clip in the gallery. */
export const OPEN_CLIP_EVENT = "clippiboy:open-clip";

/** Goes to Clips and opens the clip there. */
export function openClip(id: string): void {
  window.dispatchEvent(new CustomEvent(OPEN_CLIP_EVENT, { detail: id }));
}
