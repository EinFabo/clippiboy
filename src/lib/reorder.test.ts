import { test } from "node:test";
import assert from "node:assert/strict";
import { insertIndex, landingShift, moveTo, scrollStep, shiftOf } from "./reorder.ts";

test("insertIndex: above every middle is slot 0, below every middle the last slot", () => {
  const mids = [30, 100, 170];
  assert.equal(insertIndex(mids, 0), 0);
  assert.equal(insertIndex(mids, 29), 0);
  assert.equal(insertIndex(mids, 500), 3);
});

test("insertIndex: passing a middle moves one slot on; on the middle itself it stays", () => {
  const mids = [30, 100, 170];
  assert.equal(insertIndex(mids, 30), 0);
  assert.equal(insertIndex(mids, 31), 1);
  assert.equal(insertIndex(mids, 100), 1);
  assert.equal(insertIndex(mids, 101), 2);
  assert.equal(insertIndex(mids, 171), 3);
});

test("insertIndex: no other rows means slot 0", () => {
  assert.equal(insertIndex([], 42), 0);
});

test("moveTo: down, up, to either end", () => {
  const ids = ["a", "b", "c", "d"];
  assert.deepEqual(moveTo(ids, 0, 2), ["b", "c", "a", "d"]);
  assert.deepEqual(moveTo(ids, 3, 1), ["a", "d", "b", "c"]);
  assert.deepEqual(moveTo(ids, 1, 3), ["a", "c", "d", "b"]);
  assert.deepEqual(moveTo(ids, 2, 0), ["c", "a", "b", "d"]);
});

test("moveTo: back to its own place is no change and gives null", () => {
  assert.equal(moveTo(["a", "b", "c"], 1, 1), null);
  assert.equal(moveTo(["a"], 0, 0), null);
});

test("moveTo: a slot out of range is clamped, an unknown row is no move", () => {
  assert.deepEqual(moveTo(["a", "b", "c"], 0, 9), ["b", "c", "a"]);
  assert.equal(moveTo(["a", "b"], -1, 1), null);
  assert.equal(moveTo(["a", "b"], 5, 0), null);
});

test("moveTo leaves the list it was given alone", () => {
  const ids = ["a", "b", "c"];
  moveTo(ids, 0, 2);
  assert.deepEqual(ids, ["a", "b", "c"]);
});

test("shiftOf: moving down, the rows passed step up; others stay", () => {
  // Row 1 carried to slot 3 of five rows.
  assert.deepEqual(
    [0, 1, 2, 3, 4].map((i) => shiftOf(i, 1, 3)),
    [0, 0, -1, -1, 0],
  );
});

test("shiftOf: moving up, the rows passed step down; others stay", () => {
  // Row 3 carried to slot 0.
  assert.deepEqual(
    [0, 1, 2, 3, 4].map((i) => shiftOf(i, 3, 0)),
    [1, 1, 1, 0, 0],
  );
});

test("shiftOf: over its own slot nobody moves", () => {
  assert.deepEqual([0, 1, 2].map((i) => shiftOf(i, 1, 1)), [0, 0, 0]);
});

test("shiftOf and moveTo agree on where every row ends up", () => {
  const ids = ["a", "b", "c", "d", "e"];
  for (let from = 0; from < ids.length; from++) {
    for (let to = 0; to < ids.length; to++) {
      const next = moveTo(ids, from, to) ?? ids;
      ids.forEach((id, index) => {
        if (index === from) return;
        assert.equal(next.indexOf(id), index + shiftOf(index, from, to), `${from}→${to}, ${id}`);
      });
      assert.equal(next.indexOf(ids[from]), to);
    }
  }
});

test("landingShift: equal rows travel whole steps of height plus gap", () => {
  // Rows 60 high with a 10 px gap.
  const tops = [0, 70, 140, 210];
  const heights = [60, 60, 60, 60];
  assert.equal(landingShift(tops, heights, 0, 2), 140);
  assert.equal(landingShift(tops, heights, 3, 1), -140);
  assert.equal(landingShift(tops, heights, 2, 2), 0);
});

test("landingShift: rows of different heights land exactly where the layout puts them", () => {
  // Heights 60, 80, 50 with a 10 px gap: tops 0, 70, 160.
  const tops = [0, 70, 160];
  const heights = [60, 80, 50];
  // Row 0 to the end: the new layout is 80, 50, 60 → its top is 80+10+50+10.
  assert.equal(landingShift(tops, heights, 0, 2), 150);
  // Row 2 to the top: it takes the top of the list.
  assert.equal(landingShift(tops, heights, 2, 0), -160);
  // Row 1 down one: new layout 60, 50, 80 → its top is 60+10+50+10 = 130.
  assert.equal(landingShift(tops, heights, 1, 2), 130 - 70);
});

test("scrollStep: nothing in the middle, nothing at the zone's inner border", () => {
  assert.equal(scrollStep(400, 100, 700), 0);
  assert.equal(scrollStep(148, 100, 700), 0);
  assert.equal(scrollStep(652, 100, 700), 0);
});

test("scrollStep: faster the closer to the edge, full speed at and beyond it", () => {
  assert.equal(scrollStep(124, 100, 700), -7);
  assert.equal(scrollStep(100, 100, 700), -14);
  assert.equal(scrollStep(20, 100, 700), -14);
  assert.equal(scrollStep(676, 100, 700), 7);
  assert.equal(scrollStep(700, 100, 700), 14);
  assert.equal(scrollStep(900, 100, 700), 14);
});

test("scrollStep: an area too small for both zones never scrolls", () => {
  assert.equal(scrollStep(0, 100, 180), 0);
  assert.equal(scrollStep(300, 100, 180), 0);
});
