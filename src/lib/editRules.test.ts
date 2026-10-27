import { test } from "node:test";
import assert from "node:assert/strict";
import { isTrimmed, singleTrackVolume, snapTrim } from "./editRules.ts";

test("a cut of a quarter second or less snaps to the edge, as the core drops it", () => {
  assert.deepEqual(snapTrim(0.2, 29.9, 30), { start: 0, end: 30 });
  assert.deepEqual(snapTrim(0.3, 29.7, 30), { start: 0.3, end: 29.7 });
});
test("a quarter second is not a trim", () => {
  assert.equal(isTrimmed(0.2, 30, 30), false);
  assert.equal(isTrimmed(0.3, 30, 30), true);
  assert.equal(isTrimmed(0, 29.8, 30), false);
  assert.equal(isTrimmed(0, 29.7, 30), true);
});
test("a muted single track is silent in the preview too", () => {
  assert.equal(singleTrackVolume(1, 0, true), 0);
  assert.equal(singleTrackVolume(0.8, -6, false).toFixed(3), (0.8 * 10 ** (-6 / 20)).toFixed(3));
  assert.equal(singleTrackVolume(1, 6, false), 1, "the element cannot boost");
});
