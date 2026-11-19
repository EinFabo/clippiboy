import { test } from "node:test";
import assert from "node:assert/strict";
import { favoriteTarget, pruneSelection, rangeSelect, selectAll, toggleId } from "./clipSelect.ts";

const order = ["a", "b", "c", "d", "e"];

test("toggleId adds, removes and never changes its input", () => {
  const empty = new Set<string>();
  const one = toggleId(empty, "b");
  assert.deepEqual([...one], ["b"]);
  assert.equal(empty.size, 0);
  assert.deepEqual([...toggleId(one, "b")], []);
});

test("rangeSelect takes everything between the anchor and the target, both ends included", () => {
  assert.deepEqual([...rangeSelect(order, "b", "d", new Set())], ["b", "c", "d"]);
});

test("rangeSelect works backwards too", () => {
  assert.deepEqual([...rangeSelect(order, "d", "b", new Set())].sort(), ["b", "c", "d"]);
});

test("rangeSelect keeps what was selected before", () => {
  assert.deepEqual([...rangeSelect(order, "c", "d", new Set(["a"]))].sort(), ["a", "c", "d"]);
});

test("rangeSelect without a usable anchor selects just the target", () => {
  assert.deepEqual([...rangeSelect(order, null, "c", new Set())], ["c"]);
  assert.deepEqual([...rangeSelect(order, "gone", "c", new Set())], ["c"]);
});

test("rangeSelect with an unknown target changes nothing", () => {
  const base = new Set(["a"]);
  assert.equal(rangeSelect(order, "a", "zzz", base), base);
});

test("selectAll is the whole visible order", () => {
  assert.deepEqual([...selectAll(order)], order);
  assert.equal(selectAll([]).size, 0);
});

test("pruneSelection drops what is no longer shown", () => {
  assert.deepEqual([...pruneSelection(new Set(["a", "x", "c"]), ["a", "b", "c"])].sort(), ["a", "c"]);
});

test("pruneSelection hands back the same set when nothing dropped", () => {
  const sel = new Set(["a", "b"]);
  assert.equal(pruneSelection(sel, order), sel);
});

test("favoriteTarget removes the heart only when every clip has one", () => {
  assert.equal(favoriteTarget([{ favorite: true }, { favorite: true }]), false);
  assert.equal(favoriteTarget([{ favorite: true }, { favorite: false }]), true);
  assert.equal(favoriteTarget([{ favorite: false }]), true);
});
