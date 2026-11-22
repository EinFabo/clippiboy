import { test } from "node:test";
import assert from "node:assert/strict";
import { sameData, sameRecord } from "./same.ts";

test("two empty maps are the same", () => assert.equal(sameRecord({}, {}), true));
test("equal maps are the same", () => assert.equal(sameRecord({ a: "x", b: "y" }, { b: "y", a: "x" }), true));
test("a changed message is not", () => assert.equal(sameRecord({ a: "x" }, { a: "y" }), false));
test("an extra key is not", () => assert.equal(sameRecord({ a: "x" }, { a: "x", b: "y" }), false));
test("a different key is not", () => assert.equal(sameRecord({ a: "x" }, { b: "x" }), false));

const clip = {
  id: "a",
  title: null,
  favorite: false,
  tags: ["ace", "clutch"],
  original: { startMs: 0, endMs: 1000 },
  facecam: null,
};
test("a record read back unchanged is the same data", () =>
  assert.equal(sameData(clip, structuredClone(clip)), true));
test("a flipped field is not", () => assert.equal(sameData(clip, { ...clip, favorite: true }), false));
test("a changed nested value is not", () =>
  assert.equal(sameData(clip, { ...clip, original: { startMs: 0, endMs: 2000 } }), false));
test("a changed array is not", () => {
  assert.equal(sameData(clip, { ...clip, tags: ["ace"] }), false);
  assert.equal(sameData(clip, { ...clip, tags: ["clutch", "ace"] }), false);
});
test("null against an object is not", () =>
  assert.equal(sameData(clip, { ...clip, facecam: { state: "ready" } }), false));
test("an array is not an object with the same keys", () =>
  assert.equal(sameData(["x"], { 0: "x" }), false));
test("a missing key is not an undefined one", () =>
  assert.equal(sameData({ a: undefined }, { b: undefined }), false));
test("NaN equals NaN, like Object.is", () => assert.equal(sameData({ n: NaN }, { n: NaN }), true));

import { changedPart } from "./same.ts";

test("changedPart: an unchanged tick yields null", () => {
  const cur = { a: {}, b: [1], c: 3 };
  assert.equal(changedPart(cur, { a: {}, b: [1], c: 3 }), null);
});
test("changedPart: only the changed keys come back", () => {
  const cur = { a: { x: 1 }, b: [1], c: 3 };
  assert.deepEqual(changedPart(cur, { a: { x: 1 }, b: [2], c: 3 }), { b: [2] });
});
