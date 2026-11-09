import { test } from "node:test";
import assert from "node:assert/strict";
import { navigate, nearest, nextTab, pickInRows, pickNext, rangeSteps, type Box, type Stop } from "./spatialNav.ts";

const box = (left: number, top: number, width = 80, height = 60): Box => ({ left, top, width, height });

// A dock: six buttons in a row at the bottom.
const dock = [0, 1, 2, 3, 4, 5].map((i) => box(100 + i * 90, 900));

test("left and right follow the row", () => {
  assert.equal(pickNext(dock[2], dock, "right"), 3);
  assert.equal(pickNext(dock[2], dock, "left"), 1);
});

test("nothing that way at the end of the row", () => {
  assert.equal(pickNext(dock[5], dock, "right"), -1);
  assert.equal(pickNext(dock[0], dock, "left"), -1);
  assert.equal(pickNext(dock[0], dock, "down"), -1);
});

test("the row is followed before a nearer box above it is taken", () => {
  const above = box(290, 820);
  const all = [...dock, above];
  assert.equal(pickNext(dock[1], all, "right"), 2);
});

// A panel above the dock: a grid of three tiles, a row of small tools under each.
const tiles = [0, 1, 2].map((i) => box(100 + i * 300, 300, 280, 160));
const tools = [0, 1, 2].flatMap((t) => [0, 1, 2, 3].map((k) => box(100 + t * 300 + k * 36, 470, 28, 28)));

test("up from the dock lands in the panel, on what lies straight above", () => {
  const all = [...dock, ...tiles, ...tools];
  const picked = all[pickNext(dock[0], all, "up")];
  // The tools are nearer than the tiles; of those over the first dock
  // button, the one most in line with its centre.
  assert.deepEqual(picked, tools[1]);
});

test("down from a tile goes to its own tools, the one most in line", () => {
  const all = [...tiles, ...tools];
  const picked = pickNext(tiles[1], all, "down");
  assert.ok(picked >= 3 + 4 && picked < 3 + 8, `got ${picked}`);
});

test("right from the last tool of a tile goes to the next tile's tools, not up", () => {
  const all = [...tiles, ...tools];
  assert.equal(pickNext(tools[3], all, "right"), 3 + 4);
});

test("up from a tool goes back to its tile", () => {
  const all = [...tiles, ...tools];
  assert.equal(pickNext(tools[5], all, "up"), 1);
});

test("a box overlapping the start but centred past it still counts", () => {
  const wide = box(0, 0, 400, 40);
  const below = box(300, 30, 80, 40);
  assert.equal(pickNext(wide, [wide, below], "down"), 1);
  assert.equal(pickNext(wide, [wide], "down"), -1);
});

test("the nearest box by centre", () => {
  assert.equal(nearest(box(560, 880), dock), 5);
  assert.equal(nearest(box(0, 0), dock), 0);
  assert.equal(nearest(box(0, 0), []), -1);
});

test("a range input moves about a fortieth per press", () => {
  assert.equal(rangeSteps(-30, 12, 0.5), 2);
  assert.equal(rangeSteps(0, 1, 0.01), 3);
  assert.equal(rangeSteps(0, 10, 1), 1);
  assert.equal(rangeSteps(0, 0, 1), 1);
  assert.equal(rangeSteps(0, 10, 0), 1);
});

test("the bumpers go round the tabs", () => {
  assert.equal(nextTab(4, -1, 1), 0);
  assert.equal(nextTab(4, -1, -1), 3);
  assert.equal(nextTab(4, 1, 1), 2);
  assert.equal(nextTab(4, 3, 1), 0);
  assert.equal(nextTab(4, 0, -1), 3);
  assert.equal(nextTab(0, -1, 1), -1);
});

// The console's player as it lies on a 1920 × 1080 screen: the picture, the
// row under it (play, timeline, mute, volume), the row with the name and the
// buttons (copy, open in the app, back), and the dock below the card.
const player = {
  video: box(170, 40, 1580, 720),
  play: box(190, 780, 36, 36),
  timeline: box(242, 780, 1280, 36),
  mute: box(1610, 780, 36, 36),
  volume: box(1652, 792, 80, 12),
  copy: box(1400, 856, 90, 36),
  open: box(1500, 856, 160, 36),
  back: box(1670, 856, 80, 36),
};
const playerDock = [0, 1, 2, 3, 4, 5, 6].map((i) => box(560 + i * 92, 960, 84, 72));

/** Stops as the page hands them over: the player's in rows, the dock free. */
function playerStops(): { stops: Stop[]; names: string[] } {
  const rows: [string, number][] = [
    ["video", 0],
    ["play", 1],
    ["timeline", 1],
    ["mute", 1],
    ["volume", 1],
    ["copy", 2],
    ["open", 2],
    ["back", 2],
  ];
  const stops: Stop[] = rows.map(([name, row]) => ({
    box: player[name as keyof typeof player],
    group: 0,
    row,
  }));
  const names = rows.map(([name]) => name);
  playerDock.forEach((b, i) => {
    stops.push({ box: b });
    names.push(`dock${i}`);
  });
  return { stops, names };
}

function walk(from: string, ...dirs: ("up" | "down" | "left" | "right")[]): string {
  const { stops, names } = playerStops();
  let at = names.indexOf(from);
  for (const dir of dirs) {
    const next = navigate(stops, at, dir);
    if (next >= 0) at = next;
  }
  return names[at];
}

test("the stuck case: by geometry alone, Play is out of reach from the right", () => {
  // What the ring did before the rows: from Back, left and up never came
  // back to Play — the dock lay nearer than the far end of the card.
  const flat = [player.play, player.mute, player.volume, player.copy, player.open, player.back, ...playerDock];
  const seen = new Set<number>();
  let at = 5;
  for (let i = 0; i < 20; i += 1) {
    seen.add(at);
    const dir = i % 2 === 0 ? "left" : "up";
    const next = pickNext(flat[at], flat, dir);
    if (next >= 0 && next !== at) at = next;
  }
  assert.ok(!seen.has(0), "geometry reached Play after all — the fixture no longer shows the stuck case");
});

test("in the player, left from Back goes round the row instead of into the dock", () => {
  assert.equal(walk("back", "left"), "open");
  assert.equal(walk("back", "left", "left"), "copy");
  assert.equal(walk("back", "left", "left", "left"), "back");
  assert.equal(walk("back", "right"), "copy");
});

test("from the right side of the player Play is two kinds of step away", () => {
  assert.equal(walk("back", "up"), "volume");
  assert.equal(walk("back", "up", "left", "left", "left"), "play");
  assert.equal(walk("volume", "right"), "play");
  assert.equal(walk("copy", "up", "up"), "video");
});

test("down from the picture lands on the control under the middle of it", () => {
  assert.equal(walk("video", "down"), "timeline");
  assert.equal(walk("play", "up"), "video");
  assert.equal(walk("video", "left"), "video");
  assert.equal(walk("video", "up"), "video");
});

test("down from the buttons leaves the player for the dock, and up comes back", () => {
  assert.ok(walk("open", "down").startsWith("dock"));
  const back = walk("open", "down", "up");
  assert.ok(["copy", "open", "back"].includes(back), back);
});

test("rows: an empty row is stepped over, a missing control gives null", () => {
  const rows = [[box(0, 0)], [], [box(0, 200), box(100, 200)]];
  assert.deepEqual(pickInRows(rows, 0, 0, "down"), { row: 2, col: 0 });
  assert.equal(pickInRows(rows, 2, 1, "down"), null);
  assert.equal(pickInRows(rows, 1, 0, "left"), null);
});
