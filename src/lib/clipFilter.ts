// What the gallery shows and in which order: the filters that combine, the
// sort, the day headings, and the Game and Tags lists. Pure, so it can be
// tested.
import type { Clip } from "./types.ts";

/** The type segment. "clips" are the ones cut out of the buffer. */
export type ClipKind = "all" | "clips" | "recordings" | "screenshots";
export type ClipSort = "newest" | "oldest" | "longest" | "largest";

/**
 * Everything the gallery filters by at once: type AND favorites (if on) AND
 * any of the chosen games AND all of the chosen tags AND the search.
 */
export interface ClipFilter {
  kind: ClipKind;
  favorites: boolean;
  /** Any of these — a clip has one game. */
  games: string[];
  /** "No game" counts as one more game in the any-of above. */
  noGame: boolean;
  /** All of these — a clip can carry several. */
  tags: string[];
  query: string;
}

export const NO_FILTER: ClipFilter = {
  kind: "all",
  favorites: false,
  games: [],
  noGame: false,
  tags: [],
  query: "",
};

type Filterable = Pick<
  Clip,
  "path" | "title" | "description" | "game" | "tags" | "favorite" | "screenshot" | "recording"
>;

export function kindOf(clip: Pick<Clip, "screenshot" | "recording">): Exclude<ClipKind, "all"> {
  return clip.screenshot ? "screenshots" : clip.recording ? "recordings" : "clips";
}

function searchText(clip: Filterable): string {
  const file = clip.path.split(/[\\/]/).pop() ?? clip.path;
  return [file, clip.title ?? "", clip.description ?? "", clip.game ?? "", ...clip.tags]
    .join(" ")
    .toLowerCase();
}

export function matchesFilter(clip: Filterable, filter: ClipFilter): boolean {
  if (filter.kind !== "all" && kindOf(clip) !== filter.kind) return false;
  if (filter.favorites && !clip.favorite) return false;
  if (filter.games.length > 0 || filter.noGame) {
    const inGames = clip.game !== null && filter.games.includes(clip.game);
    if (!inGames && !(filter.noGame && !clip.game)) return false;
  }
  if (!filter.tags.every((tag) => clip.tags.includes(tag))) return false;
  const needle = filter.query.toLowerCase();
  return !needle || searchText(clip).includes(needle);
}

export function filterClips<T extends Filterable>(clips: T[], filter: ClipFilter): T[] {
  return clips.filter((clip) => matchesFilter(clip, filter));
}

/** Whether anything at all narrows the gallery — the count then says "N of M". */
export function isFiltered(filter: ClipFilter): boolean {
  return (
    filter.kind !== "all" ||
    filter.favorites ||
    filter.games.length > 0 ||
    filter.noGame ||
    filter.tags.length > 0 ||
    filter.query !== ""
  );
}

/** The counts in the type segment — over the whole library. */
export function kindCounts(clips: Array<Pick<Clip, "screenshot" | "recording">>): Record<ClipKind, number> {
  const counts = { all: clips.length, clips: 0, recordings: 0, screenshots: 0 };
  for (const clip of clips) counts[kindOf(clip)]++;
  return counts;
}

/** `name` in or out of the list. */
export function toggled(list: string[], name: string): string[] {
  return list.includes(name) ? list.filter((n) => n !== name) : [...list, name];
}

/**
 * Drops choices that no longer exist — a game renamed in the player, the last
 * favorite un-hearted. Without this the gallery would stay empty and nobody
 * would know why. Hands back the same object when nothing went, so it can sit
 * in an effect.
 */
export function pruneFilter(
  filter: ClipFilter,
  left: { games: string[]; tags: string[]; favorites: number; untagged: number },
): ClipFilter {
  const games = filter.games.filter((g) => left.games.includes(g));
  const tags = filter.tags.filter((t) => left.tags.includes(t));
  const favorites = filter.favorites && left.favorites > 0;
  const noGame = filter.noGame && left.untagged > 0;
  if (
    games.length === filter.games.length &&
    tags.length === filter.tags.length &&
    favorites === filter.favorites &&
    noGame === filter.noGame
  ) {
    return filter;
  }
  return { ...filter, games, tags, favorites, noGame };
}

type Sortable = Pick<Clip, "createdAt" | "durationMs" | "sizeBytes">;

/** A new array in the chosen order; ties go to the newer one. */
export function sortClips<T extends Sortable>(clips: T[], sort: ClipSort): T[] {
  const newer = (a: T, b: T) => b.createdAt - a.createdAt;
  const by: Record<ClipSort, (a: T, b: T) => number> = {
    newest: newer,
    oldest: (a, b) => a.createdAt - b.createdAt,
    longest: (a, b) => b.durationMs - a.durationMs || newer(a, b),
    largest: (a, b) => b.sizeBytes - a.sizeBytes || newer(a, b),
  };
  return [...clips].sort(by[sort]);
}

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** Local midnight of the day `ts` falls on. */
function midnight(ts: number): Date {
  const date = new Date(ts);
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** The local calendar day, as a stable key. */
function dayKey(ts: number): string {
  const date = new Date(ts);
  return `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`;
}

/**
 * "Today", "Yesterday", a weekday for the rest of the last seven days, then
 * "November 3" — with the year once it is not this one. Local time.
 */
export function dayLabel(ts: number, now: number): string {
  // Rounded: a day across a daylight saving change is 23 or 25 hours long.
  const days = Math.round((midnight(now).getTime() - midnight(ts).getTime()) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  const date = new Date(ts);
  if (days < 7) return WEEKDAYS[date.getDay()];
  const day = `${MONTHS[date.getMonth()]} ${date.getDate()}`;
  return date.getFullYear() === new Date(now).getFullYear() ? day : `${day}, ${date.getFullYear()}`;
}

export interface DayGroup<T> {
  key: string;
  /** `null` for the single group of a sort that has no days. */
  label: string | null;
  items: T[];
}

/**
 * The list cut into days, in the order the days first turn up — so Newest
 * leads with today and Oldest with the oldest day. Longest and Largest have no
 * days: one group without a heading. Never an empty group.
 */
export function groupByDay<T>(
  items: T[],
  createdAt: (item: T) => number,
  sort: ClipSort,
  now: number,
): Array<DayGroup<T>> {
  if (items.length === 0) return [];
  if (sort !== "newest" && sort !== "oldest") return [{ key: "all", label: null, items }];
  const groups = new Map<string, DayGroup<T>>();
  for (const item of items) {
    const ts = createdAt(item);
    const key = dayKey(ts);
    let group = groups.get(key);
    if (!group) {
      group = { key, label: dayLabel(ts, now), items: [] };
      groups.set(key, group);
    }
    group.items.push(item);
  }
  return [...groups.values()];
}

/** One entry of the Game or Tags list. */
export interface FacetValue {
  name: string;
  count: number;
}

/** Most clips first, then by name — case does not count, so "beta" sits between "Alpha" and "Gamma". */
function sortFacet(counts: Map<string, number>): FacetValue[] {
  return [...counts]
    .map(([name, count]) => ({ name, count }))
    .sort(
      (a, b) =>
        b.count - a.count ||
        a.name.localeCompare(b.name, "en", { sensitivity: "base" }) ||
        a.name.localeCompare(b.name, "en"),
    );
}

type Facetable = Pick<Clip, "game" | "tags" | "screenshot" | "recording">;

/** The clips the type segment lets through — the lists count within these. */
function ofKind<T extends Facetable>(clips: T[], kind: ClipKind): T[] {
  return kind === "all" ? clips : clips.filter((clip) => kindOf(clip) === kind);
}

/** A chosen value nothing of this type carries still gets its row, at 0 — or it could not be unticked. */
function keepChosen(counts: Map<string, number>, chosen: string[]) {
  for (const name of chosen) if (!counts.has(name)) counts.set(name, 0);
}

/**
 * The Game list: every game of the chosen type with its clips, and how many of
 * them have no game ("No game" goes last, apart from the rest).
 */
export function gameFacet(
  clips: Facetable[],
  kind: ClipKind,
  chosen: string[],
): { values: FacetValue[]; noGame: number } {
  const counts = new Map<string, number>();
  let noGame = 0;
  for (const clip of ofKind(clips, kind)) {
    if (clip.game) counts.set(clip.game, (counts.get(clip.game) ?? 0) + 1);
    else noGame++;
  }
  keepChosen(counts, chosen);
  return { values: sortFacet(counts), noGame };
}

/** The Tags list, counted like the games. */
export function tagFacet(clips: Facetable[], kind: ClipKind, chosen: string[]): FacetValue[] {
  const counts = new Map<string, number>();
  for (const clip of ofKind(clips, kind)) {
    for (const tag of clip.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  }
  keepChosen(counts, chosen);
  return sortFacet(counts);
}

/** What the search field at the top of a list leaves of it. */
export function searchFacet<T extends { name: string }>(values: T[], query: string): T[] {
  const needle = query.trim().toLowerCase();
  return needle ? values.filter((value) => value.name.toLowerCase().includes(needle)) : values;
}

/** A list gets its search field once it is longer than this. */
const FACET_SEARCH_FROM = 8;

export function facetHasSearch(entries: number): boolean {
  return entries > FACET_SEARCH_FROM;
}

/** The button's text: "Game" or "Tags" with nothing chosen, the one name, or "2 games". */
export function facetLabel(facet: "game" | "tag", chosen: string[]): string {
  if (chosen.length === 0) return facet === "game" ? "Game" : "Tags";
  if (chosen.length === 1) return chosen[0];
  return `${chosen.length} ${facet === "game" ? "games" : "tags"}`;
}
