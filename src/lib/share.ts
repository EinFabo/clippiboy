import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import { useFriends } from "./friends";
import { toast } from "./toastBus";
import { noteEnded, toDismiss, type Ended } from "./transferRows";
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
  /** More clips going to the same friend, picked together with `picking`. */
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
  const stopFriends = useFriends.subscribe((next, prev) => {
    if (next.presence !== prev.presence) check();
  });
  return () => {
    stopShare();
    stopFriends();
  };
}

/** Sends the clips of failed transfers again; each failure goes once its new
 *  transfer is on its way, and stays if that cannot start either. */
export function retryTransfers(failed: Transfer[]): void {
  for (const t of failed) {
    if (t.direction !== "out" || !t.clipId) continue;
    shareApi
      .send(t.clipId, t.friendId)
      .then(() => shareApi.dismiss(t.id))
      .catch((err) => toast("error", typeof err === "string" ? err : "Couldn't send it again."));
  }
}

/** What App listens for to open a clip in the gallery. */
export const OPEN_CLIP_EVENT = "clippiboy:open-clip";

/** Goes to Clips and opens the clip there. */
export function openClip(id: string): void {
  window.dispatchEvent(new CustomEvent(OPEN_CLIP_EVENT, { detail: id }));
}
