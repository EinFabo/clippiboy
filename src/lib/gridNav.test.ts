import { test } from "node:test";
import assert from "node:assert/strict";
import { moveInGrid } from "./gridNav.ts";

// Two day sections of 3 columns: [a b c / d] and [e f / ] and [g h i / j k l]
const sections = [["a", "b", "c", "d"], ["e", "f"], ["g", "h", "i", "j", "k", "l"]];
const go = (from: string, key: Parameters<typeof moveInGrid>[3]) =>
  moveInGrid(sections, 3, from, key);

test("left and right walk the flat order, across the day sections", () => {
  assert.equal(go("a", "ArrowRight"), "b");
  assert.equal(go("d", "ArrowRight"), "e");
  assert.equal(go("e", "ArrowLeft"), "d");
  assert.equal(go("a", "ArrowLeft"), "a");
  assert.equal(go("l", "ArrowRight"), "l");
});

test("down and up keep the column inside a section", () => {
  assert.equal(go("a", "ArrowDown"), "d");
  assert.equal(go("d", "ArrowUp"), "a");
  assert.equal(go("h", "ArrowDown"), "k");
  assert.equal(go("k", "ArrowUp"), "h");
});

test("down onto a row that is too short lands on its last tile", () => {
  assert.equal(go("b", "ArrowDown"), "d");
  assert.equal(go("c", "ArrowDown"), "d");
});

test("down from the last row enters the next section at the same column, up the last row of the previous", () => {
  assert.equal(go("d", "ArrowDown"), "e");
  assert.equal(go("f", "ArrowDown"), "h");
  assert.equal(go("i", "ArrowUp"), "f");
  assert.equal(go("g", "ArrowUp"), "e");
  assert.equal(go("e", "ArrowUp"), "d");
  assert.equal(go("f", "ArrowUp"), "d");
  assert.equal(go("a", "ArrowUp"), "a");
  assert.equal(go("l", "ArrowDown"), "l");
});

test("up into a short last row clamps to its last tile", () => {
  const s = [["a", "b", "c", "d"], ["e", "f", "g"]];
  assert.equal(moveInGrid(s, 3, "e", "ArrowUp"), "d");
  assert.equal(moveInGrid(s, 3, "g", "ArrowUp"), "d");
});

test("Home and End jump to the first and last tile", () => {
  assert.equal(go("h", "Home"), "a");
  assert.equal(go("b", "End"), "l");
});

test("an unknown tile or an empty grid answers null", () => {
  assert.equal(go("zz", "ArrowDown"), null);
  assert.equal(moveInGrid([], 3, "a", "Home"), null);
});
