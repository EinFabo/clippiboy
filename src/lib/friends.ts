import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import type { FriendsView } from "./types";

/**
 * The friends list as the core holds it. Account, lists, presence and the live
 * connection all live in `friends.rs` — the window is hidden during a game,
 * which is exactly when friends should see it. This only mirrors the view and
 * forwards what the user does.
 */
export const friendsApi = {
  signIn: () => invoke<void>("friends_sign_in"),
  cancelSignIn: () => invoke<void>("friends_cancel_sign_in"),
  signOut: () => invoke<void>("friends_sign_out"),
  refresh: () => invoke<void>("friends_refresh"),
  /** `"sent"`, or `"accepted"` when they had already asked. */
  request: (query: string) => invoke<"sent" | "accepted">("friends_request", { query }),
  accept: (id: string) => invoke<void>("friends_accept", { id }),
  /** Declines, withdraws or unfriends. */
  remove: (id: string) => invoke<void>("friends_remove", { id }),
  block: (id: string) => invoke<void>("friends_block", { id }),
  unblock: (id: string) => invoke<void>("friends_unblock", { id }),
  setAllowRequests: (allow: boolean) => invoke<void>("friends_set_allow_requests", { allow }),
  deleteAccount: () => invoke<void>("friends_delete_account"),
};

const empty: FriendsView = {
  signedIn: false,
  signingIn: false,
  connected: false,
  me: null,
  lists: { friends: [], incoming: [], outgoing: [], blocked: [] },
  presence: {},
  lastSeen: {},
};

export const useFriends = create<FriendsView>(() => empty);

if (inTauri) {
  void listen<FriendsView>("friends-state", (event) => useFriends.setState(event.payload, true));
  void invoke<FriendsView>("friends_state").then((view) => useFriends.setState(view, true));
}

/** "XXXX-XXXX" — readable over voice chat. */
export function formatCode(code: string): string {
  return code.length === 8 ? `${code.slice(0, 4)}-${code.slice(4)}` : code;
}

/** "Last seen 5 min ago", "… yesterday", "… 3 days ago". */
export function lastSeenText(at: number | undefined, now: number): string {
  if (at === undefined) return "Offline";
  const minutes = Math.max(0, Math.floor((now - at) / 60_000));
  if (minutes < 1) return "Last seen just now";
  if (minutes < 60) return `Last seen ${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Last seen ${hours} h ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "Last seen yesterday" : `Last seen ${days} days ago`;
}

/** "42 min", "1 h 5 min" since a moment in ms. */
export function playingFor(since: number | null, now: number): string | null {
  if (since === null) return null;
  const minutes = Math.max(0, Math.floor((now - since) / 60_000));
  if (minutes < 1) return "just started";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}
