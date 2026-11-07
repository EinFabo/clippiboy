import { test } from "node:test";
import assert from "node:assert/strict";
import { ShotHistory } from "./shotHistory.ts";

test("undo walks back, redo walks forward again", () => {
  const h = new ShotHistory<string>();
  h.record("a");
  h.record("ab");
  assert.equal(h.undo("abc"), "ab");
  assert.equal(h.undo("ab"), "a");
  assert.equal(h.undo("a"), null, "nothing before the start");
  assert.equal(h.redo("a"), "ab");
  assert.equal(h.redo("ab"), "abc");
  assert.equal(h.redo("abc"), null, "nothing after the newest");
});

test("a new change after an undo throws the redo steps away", () => {
  const h = new ShotHistory<string>();
  h.record("a");
  assert.equal(h.undo("ab"), "a");
  h.record("a");
  assert.equal(h.redo("ax"), null);
  assert.equal(h.canRedo, false);
});

test("one gesture is one step, a new gesture another", () => {
  const h = new ShotHistory<number>();
  h.record(0, "drag-1");
  h.record(1, "drag-1");
  h.record(2, "drag-1");
  h.record(3, "drag-2");
  assert.equal(h.undo(4), 3);
  assert.equal(h.undo(3), 0, "the whole first drag goes at once");
});

test("after an undo the same gesture does not join the step it undid", () => {
  const h = new ShotHistory<number>();
  h.record(0, "slider");
  assert.equal(h.undo(5), 0);
  h.record(0, "slider");
  assert.equal(h.undo(7), 0);
  assert.equal(h.canUndo, false);
});

test("nothing to undo means as loaded or last saved", () => {
  const h = new ShotHistory<number>();
  assert.equal(h.atStart, true);
  h.record(0);
  assert.equal(h.atStart, false);
  h.undo(1);
  assert.equal(h.atStart, true);
  h.redo(0);
  assert.equal(h.atStart, false, "redone past the saved state is unsaved again");
  h.clear();
  assert.equal(h.atStart, true);
  assert.equal(h.canRedo, false);
});
