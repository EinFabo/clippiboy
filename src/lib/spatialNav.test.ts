import { test } from "node:test";
import assert from "node:assert/strict";
import { nearest, nextTab, pickNext, rangeSteps, type Box } from "./spatialNav.ts";

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
