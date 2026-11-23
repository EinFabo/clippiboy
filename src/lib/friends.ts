import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import { isSignInReason, nextDrop, signInFailed, type Drop } from "./friendsPage";
import type { FriendPresence, FriendsView } from "./types";

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
  /** Dials the server again at once instead of after the wait. */
  reconnect: () => invoke<void>("friends_reconnect"),
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
  add: null,
};

export const useFriends = create<FriendsView>(() => empty);

if (inTauri) {
  void listen<FriendsView>("friends-state", (event) => useFriends.setState(event.payload, true));
  void invoke<FriendsView>("friends_state").then((view) => useFriends.setState(view, true));
}

/**
 * What the page knows beyond the core's view: when the hub was lost and who
 * was online until then (the core clears presence at once), and why the last
 * sign-in failed.
 */
export const useFriendsExtra = create<{ drop: Drop | null; signInError: string | null }>(() => ({
  drop: null,
  signInError: null,
}));

/** Cancel was pressed — the waiting flag going away is then no failure. */
let signInCancelled = false;
/** The last error notice from the core, which is where a failed sign-in says why. */
let lastError: { message: string; at: number } | null = null;
/** When the last sign-in failed — its notice comes just after the view. */
let failedAt = 0;

useFriends.subscribe((next, prev) => {
  const drop = nextDrop(prev, next, Date.now(), useFriendsExtra.getState().drop);
  if (drop !== useFriendsExtra.getState().drop) useFriendsExtra.setState({ drop });
  if (signInFailed(prev, next, signInCancelled)) {
    failedAt = Date.now();
    const recent = lastError && failedAt - lastError.at < 3_000 ? lastError.message : null;
    useFriendsExtra.setState({ signInError: recent ?? "The sign-in didn't finish." });
  }
  if (!next.signingIn) signInCancelled = false;
  if (next.signedIn && useFriendsExtra.getState().signInError) useFriendsExtra.setState({ signInError: null });
});

if (inTauri) {
  void listen<{ kind: string; message: string }>("notice", (event) => {
    if (event.payload.kind !== "error") return;
    lastError = { message: event.payload.message, at: Date.now() };
    if (isSignInReason(event.payload, failedAt, lastError.at, useFriends.getState().signedIn)) {
      useFriendsExtra.setState({ signInError: event.payload.message });
    }
  });
}

/** Sign-in cards on screen; while one is, it says why a sign-in failed. */
let signInCards = 0;

/** A sign-in card shows; the returned function says it went. */
export function showSignInCard(): () => void {
  signInCards += 1;
  return () => {
    signInCards -= 1;
  };
}

/**
 * Is this core notice the reason a sign-in just failed, with the card there to
 * show it? Then the toast keeps quiet — the reason was said twice.
 */
export function signInCardSays(notice: { kind: string }): boolean {
  return signInCards > 0 && isSignInReason(notice, failedAt, Date.now(), useFriends.getState().signedIn);
}

/** "Sign in with Discord" and "Open browser again": the browser opens, the core waits. */
export function startSignIn(): void {
  signInCancelled = false;
  failedAt = 0;
  useFriendsExtra.setState({ signInError: null });
  friendsApi.signIn().catch((err) => useFriendsExtra.setState({ signInError: String(err) }));
}

/** "Cancel" while waiting for Discord. */
export function cancelSignIn(): void {
  signInCancelled = true;
  void friendsApi.cancelSignIn();
}

/** Can a clip go to them right now? Online and not busy — like Quick Share,
 *  there is no mailbox for the others. */
export function canReceive(presence: FriendPresence | null | undefined): boolean {
  return !!presence && !presence.busy;
}

/** Someone's own invite link: "Add <name> as friend" on clippiboy.com. */
export function inviteLink(code: string): string {
  return `https://clippiboy.com/i/${code}`;
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
