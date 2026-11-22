import { test } from "node:test";
import assert from "node:assert/strict";
import {
  RECENT_CLIPS,
  filterClips,
  friendPickRows,
  pickSummary,
  pickedClips,
  pickerChips,
  sameGame,
  togglePick,
} from "./sendPicker.ts";
import type { Clip, FriendPresence, FriendUser } from "./types.ts";

const clip = (id: string, createdAt: number, over: Partial<Clip> = {}): Clip =>
  ({ id, createdAt, game: null, favorite: false, sizeBytes: 1_048_576, ...over }) as Clip;

const library = [
  clip("old", 1, { game: "Valorant" }),
  clip("cs1", 5, { game: "Counter-Strike 2", favorite: true }),
  clip("cs2", 9, { game: "counter-strike 2 " }),
  clip("eld", 7, { game: "Elden Ring" }),
];

test("picker: chips filter newest first", () => {
  assert.deepEqual(filterClips(library, "recent", null).map((c) => c.id), ["cs2", "eld", "cs1", "old"]);
  assert.deepEqual(filterClips(library, "game", "Counter-Strike 2").map((c) => c.id), ["cs2", "cs1"]);
  assert.deepEqual(filterClips(library, "favorites", null).map((c) => c.id), ["cs1"]);
  const many = Array.from({ length: 20 }, (_, i) => clip(`c${i}`, i));
  assert.equal(filterClips(many, "recent", null).length, RECENT_CLIPS);
  assert.equal(filterClips(many, "recent", null)[0].id, "c19");
  assert.equal(sameGame(null, null), false);
});

test("picker: a chip only when it has clips, none when Recent would stand alone", () => {
  assert.deepEqual(pickerChips(library, "Counter-Strike 2").map((c) => c.label), ["Recent", "Counter-Strike 2", "Favorites"]);
  assert.deepEqual(pickerChips(library, "Minecraft").map((c) => c.id), ["recent", "favorites"]);
  assert.deepEqual(pickerChips(library, null).map((c) => c.id), ["recent", "favorites"]);
  assert.deepEqual(pickerChips([clip("a", 1)], "Valorant"), []);
});

test("picker: picks in the order made, summed for the footer", () => {
  let picked = togglePick([], "cs2");
  picked = togglePick(picked, "old");
  picked = togglePick(picked, "eld");
  picked = togglePick(picked, "old");
  assert.deepEqual(picked, ["cs2", "eld"]);
  // A clip deleted meanwhile drops out.
  const clips = pickedClips(library, [...picked, "gone"]);
  assert.deepEqual(clips.map((c) => c.id), ["cs2", "eld"]);
  assert.equal(pickSummary(clips, (b) => `${b / 1_048_576} MB`), "2 selected · 2 MB");
  assert.equal(pickSummary([], String), "");
});

test("picker: everyone listed, busy greyed after the reachable, offline below", () => {
  const u = (id: string): FriendUser => ({ id, username: id, displayName: id, avatar: null });
  const p = (over: Partial<FriendPresence> = {}): FriendPresence => ({ game: null, since: null, status: null, busy: false, ...over });
  const friends = ["sophie", "jonas", "luca", "matti", "ben", "lea"].map(u);
  const presence = {
    sophie: p({ busy: true }),
    jonas: p({ status: "back at 8" }),
    luca: p({ game: "Counter-Strike 2" }),
    matti: p({ game: "Valorant" }),
  };
  const rows = friendPickRows(friends, presence, { ben: 50, lea: 10 }, ["matti"]);
  assert.deepEqual(
    rows.online.map((r) => [r.friend.id, r.reachable]),
    [
      ["matti", true],
      ["luca", true],
      ["jonas", true],
      ["sophie", false],
    ],
  );
  assert.deepEqual(rows.offline.map((f) => f.id), ["ben", "lea"]);
  // Nobody online: the same list, all of it below.
  const none = friendPickRows(friends, {}, {}, []);
  assert.deepEqual(none.online, []);
  assert.equal(none.offline.length, 6);
});
