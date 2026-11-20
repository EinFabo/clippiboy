import { test } from "node:test";
import assert from "node:assert/strict";
import {
  NO_FILTER,
  dayLabel,
  filterClips,
  facetHasSearch,
  facetLabel,
  gameFacet,
  searchFacet,
  tagFacet,
  groupByDay,
  isFiltered,
  kindCounts,
  pruneFilter,
  sortClips,
  toggled,
  type ClipFilter,
} from "./clipFilter.ts";

/** Just what the filter looks at, with defaults for the rest. */
function clip(id: string, over: Partial<Sample> = {}): Sample {
  return {
    id,
    path: `C:\\Videos\\${id}.mp4`,
    title: null,
    description: null,
    game: null,
    tags: [],
    favorite: false,
    screenshot: false,
    recording: false,
    createdAt: 0,
    durationMs: 0,
    sizeBytes: 0,
    ...over,
  };
}
interface Sample {
  id: string;
  path: string;
  title: string | null;
  description: string | null;
  game: string | null;
  tags: string[];
  favorite: boolean;
  screenshot: boolean;
  recording: boolean;
  createdAt: number;
  durationMs: number;
  sizeBytes: number;
}
const ids = (list: Sample[]) => list.map((c) => c.id);
const f = (over: Partial<ClipFilter>): ClipFilter => ({ ...NO_FILTER, ...over });

const library = [
  clip("cut-cs", { game: "CS2", tags: ["ace", "clutch"], favorite: true }),
  clip("cut-val", { game: "Valorant", tags: ["clutch"] }),
  clip("cut-none", {}),
  clip("rec-cs", { game: "CS2", recording: true, tags: ["ace"] }),
  clip("shot-cs", { game: "CS2", screenshot: true, title: "Scoreboard" }),
  clip("shot-none", { screenshot: true, favorite: true }),
];

// --- filter ---------------------------------------------------------------

test("no filter shows all three kinds", () => {
  assert.deepEqual(ids(filterClips(library, NO_FILTER)), ids(library));
  assert.equal(isFiltered(NO_FILTER), false);
});
test("the type segment picks one kind", () => {
  assert.deepEqual(ids(filterClips(library, f({ kind: "clips" }))), ["cut-cs", "cut-val", "cut-none"]);
  assert.deepEqual(ids(filterClips(library, f({ kind: "recordings" }))), ["rec-cs"]);
  assert.deepEqual(ids(filterClips(library, f({ kind: "screenshots" }))), ["shot-cs", "shot-none"]);
});
test("games are any-of, and No game joins them", () => {
  assert.deepEqual(ids(filterClips(library, f({ games: ["CS2", "Valorant"] }))), [
    "cut-cs",
    "cut-val",
    "rec-cs",
    "shot-cs",
  ]);
  assert.deepEqual(ids(filterClips(library, f({ games: ["Valorant"], noGame: true }))), [
    "cut-val",
    "cut-none",
    "shot-none",
  ]);
  assert.deepEqual(ids(filterClips(library, f({ noGame: true }))), ["cut-none", "shot-none"]);
});
test("tags are all-of", () => {
  assert.deepEqual(ids(filterClips(library, f({ tags: ["clutch"] }))), ["cut-cs", "cut-val"]);
  assert.deepEqual(ids(filterClips(library, f({ tags: ["clutch", "ace"] }))), ["cut-cs"]);
});
test("everything combines: type and favorites and game and tags and search", () => {
  assert.deepEqual(ids(filterClips(library, f({ kind: "clips", favorites: true }))), ["cut-cs"]);
  assert.deepEqual(ids(filterClips(library, f({ favorites: true, games: ["CS2"] }))), ["cut-cs"]);
  assert.deepEqual(ids(filterClips(library, f({ games: ["CS2"], tags: ["ace"], kind: "recordings" }))), [
    "rec-cs",
  ]);
  assert.deepEqual(ids(filterClips(library, f({ games: ["CS2"], query: "score" }))), ["shot-cs"]);
  assert.deepEqual(ids(filterClips(library, f({ kind: "clips", query: "score" }))), []);
});
test("search looks at file name, title, description, game and tags, ignoring case", () => {
  assert.deepEqual(ids(filterClips(library, f({ query: "SHOT-NONE" }))), ["shot-none"]);
  assert.deepEqual(ids(filterClips(library, f({ query: "valo" }))), ["cut-val"]);
  assert.deepEqual(ids(filterClips(library, f({ query: "clutch" }))), ["cut-cs", "cut-val"]);
  const described = [clip("d", { description: "1v3 after the retake" })];
  assert.deepEqual(ids(filterClips(described, f({ query: "retake" }))), ["d"]);
});
test("any choice counts as filtered", () => {
  for (const over of [
    { kind: "clips" as const },
    { favorites: true },
    { games: ["CS2"] },
    { noGame: true },
    { tags: ["ace"] },
    { query: "x" },
  ]) {
    assert.equal(isFiltered(f(over)), true, JSON.stringify(over));
  }
});
test("counts per kind are over the whole library", () => {
  assert.deepEqual(kindCounts(library), { all: 6, clips: 3, recordings: 1, screenshots: 2 });
  assert.deepEqual(kindCounts([]), { all: 0, clips: 0, recordings: 0, screenshots: 0 });
});
test("toggling adds and removes a name", () => {
  assert.deepEqual(toggled(["a"], "b"), ["a", "b"]);
  assert.deepEqual(toggled(["a", "b"], "a"), ["b"]);
});
test("a choice that no longer exists drops out, the rest stays", () => {
  const before = f({ kind: "clips", favorites: true, games: ["CS2", "Gone"], noGame: true, tags: ["ace", "old"] });
  const after = pruneFilter(before, { games: ["CS2"], tags: ["ace"], favorites: 0, untagged: 0 });
  assert.deepEqual(after, f({ kind: "clips", games: ["CS2"], tags: ["ace"] }));
});
test("pruning a filter that is still whole hands back the same object", () => {
  const whole = f({ games: ["CS2"], favorites: true });
  assert.equal(pruneFilter(whole, { games: ["CS2"], tags: [], favorites: 1, untagged: 0 }), whole);
});

// --- sort -----------------------------------------------------------------

const timed = [
  clip("a", { createdAt: 300, durationMs: 10, sizeBytes: 5 }),
  clip("b", { createdAt: 100, durationMs: 90, sizeBytes: 50 }),
  clip("c", { createdAt: 200, durationMs: 90, sizeBytes: 500 }),
  clip("still", { createdAt: 250, durationMs: 0, sizeBytes: 1, screenshot: true }),
];
test("newest and oldest go by creation time", () => {
  assert.deepEqual(ids(sortClips(timed, "newest")), ["a", "still", "c", "b"]);
  assert.deepEqual(ids(sortClips(timed, "oldest")), ["b", "c", "still", "a"]);
});
test("longest puts stills last, a tie goes to the newer one", () => {
  assert.deepEqual(ids(sortClips(timed, "longest")), ["c", "b", "a", "still"]);
});
test("largest goes by size", () => {
  assert.deepEqual(ids(sortClips(timed, "largest")), ["c", "b", "a", "still"]);
});
test("sorting leaves the input alone", () => {
  const copy = [...timed];
  sortClips(timed, "oldest");
  assert.deepEqual(timed, copy);
});

// --- days -----------------------------------------------------------------

/** Local time, the way the gallery reads `createdAt`. */
const at = (y: number, m: number, d: number, h = 12, min = 0) => new Date(y, m - 1, d, h, min).getTime();
// Thursday, 19 November 2026, mid-afternoon.
const now = at(2026, 11, 19, 15, 30);

test("today and yesterday split at local midnight", () => {
  assert.equal(dayLabel(at(2026, 11, 19, 0, 0), now), "Today");
  assert.equal(dayLabel(at(2026, 11, 18, 23, 59), now), "Yesterday");
  assert.equal(dayLabel(at(2026, 11, 18, 0, 0), now), "Yesterday");
  assert.equal(dayLabel(at(2026, 11, 17, 23, 59), now), "Tuesday");
});
test("shortly after midnight, an hour ago is already yesterday", () => {
  const justAfter = at(2026, 11, 19, 0, 5);
  assert.equal(dayLabel(at(2026, 11, 18, 23, 5), justAfter), "Yesterday");
  assert.equal(dayLabel(at(2026, 11, 19, 0, 1), justAfter), "Today");
});
test("weekday names for the rest of the last seven days, dates after that", () => {
  assert.equal(dayLabel(at(2026, 11, 16), now), "Monday");
  assert.equal(dayLabel(at(2026, 11, 13), now), "Friday");
  // Seven days back is the same weekday as today: a date says which one.
  assert.equal(dayLabel(at(2026, 11, 12), now), "November 12");
  assert.equal(dayLabel(at(2026, 11, 3), now), "November 3");
});
test("a date from another year carries the year", () => {
  assert.equal(dayLabel(at(2025, 11, 3), now), "November 3, 2025");
  const newYear = at(2027, 1, 2, 9);
  assert.equal(dayLabel(at(2027, 1, 1), newYear), "Yesterday");
  assert.equal(dayLabel(at(2026, 12, 31), newYear), "Thursday");
  assert.equal(dayLabel(at(2026, 12, 20), newYear), "December 20, 2026");
});
test("a day with a clock change in it is still one day", () => {
  // Europe moves its clocks back on 25 October 2026, the US on 1 November.
  assert.equal(dayLabel(at(2026, 10, 25, 0, 30), at(2026, 10, 26, 12)), "Yesterday");
  assert.equal(dayLabel(at(2026, 11, 1, 0, 30), at(2026, 11, 2, 0, 10)), "Yesterday");
  assert.equal(dayLabel(at(2026, 3, 29, 0, 30), at(2026, 4, 4, 23, 50)), "Sunday");
});
test("a clock that runs ahead counts as today", () => {
  assert.equal(dayLabel(at(2026, 11, 20, 9), now), "Today");
});

test("newest groups by day, newest day first, each day in list order", () => {
  const list = [
    clip("t2", { createdAt: at(2026, 11, 19, 14) }),
    clip("t1", { createdAt: at(2026, 11, 19, 1) }),
    clip("y", { createdAt: at(2026, 11, 18, 20) }),
    clip("old", { createdAt: at(2025, 6, 1) }),
  ];
  const groups = groupByDay(list, (c) => c.createdAt, "newest", now);
  assert.deepEqual(
    groups.map((g) => [g.label, ids(g.items)]),
    [
      ["Today", ["t2", "t1"]],
      ["Yesterday", ["y"]],
      ["June 1, 2025", ["old"]],
    ],
  );
});
test("oldest runs the groups oldest first", () => {
  const list = sortClips(
    [
      clip("t", { createdAt: at(2026, 11, 19, 9) }),
      clip("y", { createdAt: at(2026, 11, 18, 9) }),
    ],
    "oldest",
  );
  assert.deepEqual(
    groupByDay(list, (c) => c.createdAt, "oldest", now).map((g) => g.label),
    ["Yesterday", "Today"],
  );
});
test("no headings when sorted by length or size", () => {
  const list = [clip("a", { createdAt: at(2026, 11, 19) }), clip("b", { createdAt: at(2026, 11, 1) })];
  for (const sort of ["longest", "largest"] as const) {
    const groups = groupByDay(list, (c) => c.createdAt, sort, now);
    assert.equal(groups.length, 1);
    assert.equal(groups[0].label, null);
    assert.deepEqual(ids(groups[0].items), ["a", "b"]);
  }
});
test("empty in, no groups out — not even an empty one", () => {
  assert.deepEqual(groupByDay([], (c: Sample) => c.createdAt, "newest", now), []);
  assert.deepEqual(groupByDay([], (c: Sample) => c.createdAt, "largest", now), []);
});
test("an item out of order still lands in its own day, never in a second group of it", () => {
  // A tile on its way out keeps its old slot, which may sit between two days.
  const list = [
    clip("t", { createdAt: at(2026, 11, 19, 9) }),
    clip("y", { createdAt: at(2026, 11, 18, 9) }),
    clip("t-leaving", { createdAt: at(2026, 11, 19, 8) }),
  ];
  const groups = groupByDay(list, (c) => c.createdAt, "newest", now);
  assert.deepEqual(
    groups.map((g) => [g.label, ids(g.items)]),
    [
      ["Today", ["t", "t-leaving"]],
      ["Yesterday", ["y"]],
    ],
  );
  assert.equal(new Set(groups.map((g) => g.key)).size, groups.length);
});

// --- the Game and Tags lists ---------------------------------------------

test("game list: counts within the chosen type, most clips first, then by name", () => {
  const clips = [
    clip("a", { game: "valorant" }),
    clip("b", { game: "CS2" }),
    clip("c", { game: "CS2" }),
    clip("d", { game: "Apex" }),
    clip("e", { game: "Valorant", screenshot: true }),
    clip("f", {}),
    clip("g", { screenshot: true }),
  ];
  assert.deepEqual(gameFacet(clips, "all", []), {
    values: [
      { name: "CS2", count: 2 },
      { name: "Apex", count: 1 },
      { name: "valorant", count: 1 },
      { name: "Valorant", count: 1 },
    ],
    noGame: 2,
  });
  assert.deepEqual(gameFacet(clips, "screenshots", []), {
    values: [{ name: "Valorant", count: 1 }],
    noGame: 1,
  });
  assert.deepEqual(gameFacet(clips, "recordings", []), { values: [], noGame: 0 });
});

test("names sort without regard to case", () => {
  const clips = [clip("a", { game: "beta" }), clip("b", { game: "Alpha" }), clip("c", { game: "Gamma" })];
  assert.deepEqual(
    gameFacet(clips, "all", []).values.map((v) => v.name),
    ["Alpha", "beta", "Gamma"],
  );
});

test("a chosen value outside the type stays in the list with 0, so it can be unticked", () => {
  const clips = [clip("a", { game: "CS2" }), clip("b", { game: "Valorant", screenshot: true, tags: ["ace"] })];
  assert.deepEqual(gameFacet(clips, "clips", ["Valorant"]).values, [
    { name: "CS2", count: 1 },
    { name: "Valorant", count: 0 },
  ]);
  assert.deepEqual(tagFacet(clips, "clips", ["ace"]), [{ name: "ace", count: 0 }]);
});

test("tag list: every tag of a clip counts, within the type", () => {
  assert.deepEqual(tagFacet(library, "all", []), [
    { name: "ace", count: 2 },
    { name: "clutch", count: 2 },
  ]);
  assert.deepEqual(tagFacet(library, "recordings", []), [{ name: "ace", count: 1 }]);
  assert.deepEqual(tagFacet(library, "screenshots", []), []);
});

test("search in a list: part of the name, any case", () => {
  const values = [
    { name: "Counter-Strike 2", count: 3 },
    { name: "Valorant", count: 2 },
    { name: "Rocket League", count: 1 },
  ];
  assert.deepEqual(searchFacet(values, "").map((v) => v.name), ["Counter-Strike 2", "Valorant", "Rocket League"]);
  assert.deepEqual(searchFacet(values, "  ").length, 3);
  assert.deepEqual(searchFacet(values, "STRIKE").map((v) => v.name), ["Counter-Strike 2"]);
  assert.deepEqual(searchFacet(values, "e").map((v) => v.name), ["Counter-Strike 2", "Rocket League"]);
  assert.deepEqual(searchFacet(values, "xyz"), []);
});

test("the search field comes with more than eight entries", () => {
  assert.equal(facetHasSearch(8), false);
  assert.equal(facetHasSearch(9), true);
});

test("button label: the facet, the one name, or how many", () => {
  assert.equal(facetLabel("game", []), "Game");
  assert.equal(facetLabel("tag", []), "Tags");
  assert.equal(facetLabel("game", ["Counter-Strike 2"]), "Counter-Strike 2");
  assert.equal(facetLabel("tag", ["clutch"]), "clutch");
  assert.equal(facetLabel("game", ["CS2", "No game"]), "2 games");
  assert.equal(facetLabel("tag", ["ace", "clutch", "funny"]), "3 tags");
});
