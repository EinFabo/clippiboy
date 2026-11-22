import { groupFriends } from "./friendsList.ts";
import type { Clip, FriendPresence, FriendUser } from "./types";

/**
 * The two ways to send a clip, apart from the screen: from a friend (which
 * clips, filtered and picked several at once) and from a clip (which friends,
 * the ones who can't take it right now greyed rather than left out).
 */

/** How many clips "Recent" offers — what one sends is nearly always new. */
export const RECENT_CLIPS = 12;

export type ClipFilter = "recent" | "game" | "favorites";

export interface PickerChip {
  id: ClipFilter;
  label: string;
}

/** Whether a clip is from the game a friend plays — names as each side saw them. */
export function sameGame(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false;
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

const newestFirst = (clips: Clip[]) => [...clips].sort((a, b) => b.createdAt - a.createdAt);

/** The clips under a chip, newest first. */
export function filterClips(clips: Clip[], filter: ClipFilter, game: string | null): Clip[] {
  const sorted = newestFirst(clips);
  if (filter === "game") return sorted.filter((clip) => sameGame(clip.game, game));
  if (filter === "favorites") return sorted.filter((clip) => clip.favorite);
  return sorted.slice(0, RECENT_CLIPS);
}

/**
 * Recent, the game the friend is playing, Favorites — each only when it has
 * clips. A lone "Recent" is no choice, so then there are no chips at all.
 */
export function pickerChips(clips: Clip[], game: string | null): PickerChip[] {
  const chips: PickerChip[] = [{ id: "recent", label: "Recent" }];
  if (game && clips.some((clip) => sameGame(clip.game, game))) chips.push({ id: "game", label: game });
  if (clips.some((clip) => clip.favorite)) chips.push({ id: "favorites", label: "Favorites" });
  return chips.length > 1 ? chips : [];
}

/** A tile in or out of the selection; the order picked is the order sent. */
export function togglePick(selected: readonly string[], id: string): string[] {
  return selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id];
}

/** The picked clips still in the library, in the order picked. */
export function pickedClips(clips: Clip[], selected: readonly string[]): Clip[] {
  const byId = new Map(clips.map((clip) => [clip.id, clip]));
  return selected.flatMap((id) => byId.get(id) ?? []);
}

/** "2 selected · 94 MB"; nothing while none is picked. */
export function pickSummary(picked: Clip[], size: (bytes: number) => string): string {
  if (picked.length === 0) return "";
  return `${picked.length} selected · ${size(picked.reduce((sum, clip) => sum + clip.sizeBytes, 0))}`;
}

/** One friend in the clip's picker. */
export interface PickRow {
  friend: FriendUser;
  /** Online and not busy — only then can a clip go. */
  reachable: boolean;
}

/**
 * Everyone, in the Friends page's order: who is playing, who is online — the
 * busy ones at the end of those, greyed — and below them the offline ones by
 * when they were last seen.
 */
export function friendPickRows(
  friends: FriendUser[],
  presence: Record<string, FriendPresence>,
  lastSeen: Record<string, number>,
  favorites: string[],
): { online: PickRow[]; offline: FriendUser[] } {
  const groups = groupFriends(friends, presence, lastSeen, favorites);
  const online = [...groups.playing, ...groups.online].map((friend) => ({
    friend,
    reachable: !presence[friend.id].busy,
  }));
  return {
    online: [...online.filter((row) => row.reachable), ...online.filter((row) => !row.reachable)],
    offline: groups.offline,
  };
}
