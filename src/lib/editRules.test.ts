import { test } from "node:test";
import assert from "node:assert/strict";
import { asksBeforeTrim, isTrimmed, sameCut, singleTrackVolume, snapTrim } from "./editRules.ts";

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

test("a real move of the cut is a change, an edge cut under a quarter second is not", () => {
  assert.equal(sameCut({ start: 0.4, end: 30 }, { start: 0.55, end: 30 }, 30), false);
  assert.equal(sameCut({ start: 0.1, end: 30 }, { start: 0, end: 30 }, 30), true);
  assert.equal(sameCut({ start: 0, end: 29.9 }, { start: 0, end: 30 }, 30), true);
  assert.equal(sameCut({ start: 0.4, end: 30 }, { start: 0.42, end: 30 }, 30), true);
});
test("a track at or below the core's silence threshold is silent", () => {
  assert.equal(singleTrackVolume(1, -59, false), 0);
  assert.ok(singleTrackVolume(1, -58, false) > 0);
});

test("a recording asks before a trim, a clip does not", () => {
  const recording = { recording: true, originalAvailable: false };
  const clip = { recording: false, originalAvailable: false };
  assert.equal(asksBeforeTrim(recording, 5, 20, 30), true);
  assert.equal(asksBeforeTrim(clip, 5, 20, 30), false);
  assert.equal(asksBeforeTrim(recording, 0.1, 29.9, 30), false, "no cut, only the mix");
  assert.equal(
    asksBeforeTrim({ recording: true, originalAvailable: true }, 5, 20, 30),
    false,
    "an older recording whose copy is still there can still be undone",
  );
});
