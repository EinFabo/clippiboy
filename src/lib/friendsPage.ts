import type { FriendUser } from "./types";

/**
 * The Friends page's own rules, apart from the page so they can be tested:
 * the status chip and its menu, the Add friend answer, the friend code as the
 * screen shows it, and what is known of friends while the hub is unreachable.
 */

/** What the status chip shows. `connecting` while the hub cannot be reached. */
export type MyStatus = "online" | "busy" | "invisible" | "connecting";

/** The three a user can pick in the status menu. */
export type StatusChoice = Exclude<MyStatus, "connecting">;

export const STATUS_LABEL: Record<MyStatus, string> = {
  online: "Online",
  busy: "Busy",
  invisible: "Appear offline",
  connecting: "Connecting…",
};

/** The chip's state from the settings and the connection. Appear offline
 *  wins over Busy, as the core sends no presence at all then. */
export function myStatus(config: { busy: boolean; invisible: boolean }, connected: boolean): MyStatus {
  if (!connected) return "connecting";
  if (config.invisible) return "invisible";
  return config.busy ? "busy" : "online";
}

/** The menu's current pick — the same as the chip, minus the connection. */
export function statusChoice(config: { busy: boolean; invisible: boolean }): StatusChoice {
  return config.invisible ? "invisible" : config.busy ? "busy" : "online";
}

/** The status line as it is saved: trimmed, at most 60 characters. */
export function statusText(text: string): string {
  return text.trim().slice(0, 60);
}

/**
 * The settings a pick in the menu writes. Exactly one of the two is on, or
 * neither. The status line typed in the menu goes along in the same patch —
 * a second patch from the same snapshot would undo the pick.
 */
export function statusPatch(choice: StatusChoice): { busy: boolean; invisible: boolean };
export function statusPatch(choice: StatusChoice, text: string): { busy: boolean; invisible: boolean; status: string };
export function statusPatch(choice: StatusChoice, text?: string) {
  const patch = { busy: choice === "busy", invisible: choice === "invisible" };
  return text === undefined ? patch : { ...patch, status: statusText(text) };
}

/** What the server answers when there is nothing to do — said calmly, not in red. */
const ALREADY = ["You are already friends.", "Request already sent."];

export type AddOutcome =
  | { ok: true; status: "sent" | "accepted" }
  | { ok: false; error: string };

export interface AddAnswer {
  tone: "ok" | "info" | "bad";
  text: string;
}

/**
 * The line under the field after "Add". `name` is who turned up in the lists
 * because of it, once the core's refresh has arrived; until then the answer
 * reads without a name. Refusals are the server's own words.
 */
export function addAnswer(outcome: AddOutcome, name: string | null): AddAnswer {
  if (outcome.ok) {
    if (outcome.status === "accepted") {
      return {
        tone: "ok",
        text: name ? `${name} had asked you already. You are friends now.` : "They had asked you already. You are friends now.",
      };
    }
    return { tone: "ok", text: name ? `Request sent to ${name}.` : "Request sent." };
  }
  const error = outcome.error.trim() || "Something went wrong.";
  return { tone: ALREADY.includes(error) ? "info" : "bad", text: error };
}

/** Who is new in `users` since `before` (ids) — the one a request just reached. */
export function newcomer(before: ReadonlySet<string>, users: readonly FriendUser[]): FriendUser | null {
  return users.find((user) => !before.has(user.id)) ?? null;
}

/** "clippiboy.com/i/K7Q2M9XD" — the invite link without its scheme, for the screen. */
export function shortLink(code: string): string {
  return `clippiboy.com/i/${code}`;
}

/** The code's shape without the code, for streamer mode: "••••-••••". */
export function maskedCode(code: string): string {
  const dots = (n: number) => "•".repeat(n);
  return code.length === 8 ? `${dots(4)}-${dots(4)}` : dots(code.length);
}

/** When the hub was lost and who was online right before. */
export interface Drop {
  at: number;
  online: string[];
}

/**
 * Follows the connection from one view to the next: a drop is noted with the
 * friends that were online until then — the core clears presence the moment
 * the hub goes, so this is the only place that still knows. Reconnecting
 * forgets it.
 */
export function nextDrop(
  prev: { connected: boolean; presence: Record<string, unknown> },
  next: { connected: boolean },
  now: number,
  current: Drop | null,
): Drop | null {
  if (next.connected) return null;
  if (prev.connected) return { at: now, online: Object.keys(prev.presence) };
  return current;
}

/**
 * When a friend was last known to be around while the hub is unreachable:
 * those online at the drop were seen then, the others as the hub last said.
 */
export function seenWhileAway(
  id: string,
  lastSeen: Record<string, number>,
  drop: Drop | null,
): number | undefined {
  if (drop?.online.includes(id)) return drop.at;
  return lastSeen[id];
}

/** The greyed list while unreachable: the most recently seen first, then by name. */
export function awayOrder(
  friends: readonly FriendUser[],
  lastSeen: Record<string, number>,
  drop: Drop | null,
): FriendUser[] {
  return [...friends].sort((a, b) => {
    const at = seenWhileAway(a.id, lastSeen, drop) ?? -Infinity;
    const bt = seenWhileAway(b.id, lastSeen, drop) ?? -Infinity;
    if (at !== bt) return bt - at;
    return a.displayName.localeCompare(b.displayName);
  });
}

/**
 * Did a sign-in just end without succeeding? The core only drops the
 * "waiting" flag; a failure is that flag going away while still signed out,
 * unless the user pressed Cancel.
 */
export function signInFailed(
  prev: { signingIn: boolean },
  next: { signingIn: boolean; signedIn: boolean },
  cancelled: boolean,
): boolean {
  return prev.signingIn && !next.signingIn && !next.signedIn && !cancelled;
}
