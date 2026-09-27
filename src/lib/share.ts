import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import type { Clip, Transfer } from "./types";

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
  pick: (clip: Clip | null) => void;
}

export const useShare = create<ShareState>((set) => ({
  transfers: [],
  picking: null,
  pick: (clip) => set({ picking: clip }),
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
