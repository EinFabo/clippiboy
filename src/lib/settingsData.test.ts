import { test } from "node:test";
import assert from "node:assert/strict";
import {
  bakeCardShown,
  cameraMissing,
  isStale,
  parseStorageCache,
  readStillCurrent,
  STORAGE_MAX_AGE_MS,
} from "./settingsData.ts";

const usage = {
  clipsBytes: 1, originalsBytes: 2, facecamBytes: 3, shotEditsBytes: 4, tracksBytes: 5,
  thumbsBytes: 6, cacheBytes: 7, toolsBytes: 8, trashBytes: 9,
};

test("isStale: never read, old, or from the future is stale", () => {
  assert.equal(isStale(null, 1000, STORAGE_MAX_AGE_MS), true);
  assert.equal(isStale(1000, 1000 + STORAGE_MAX_AGE_MS, STORAGE_MAX_AGE_MS), false);
  assert.equal(isStale(1000, 1001 + STORAGE_MAX_AGE_MS, STORAGE_MAX_AGE_MS), true);
  assert.equal(isStale(5000, 1000, STORAGE_MAX_AGE_MS), true);
  assert.equal(isStale(NaN, 1000, STORAGE_MAX_AGE_MS), true);
});

test("parseStorageCache: round trip and rejects", () => {
  const cache = { usage, trims: { count: 2, bytes: 10 }, defaultDir: "C:\\Clips", at: 123 };
  assert.deepEqual(parseStorageCache(JSON.stringify(cache)), cache);
  assert.deepEqual(parseStorageCache(JSON.stringify({ trims: { count: 1, bytes: 2 } })), {
    usage: null, trims: { count: 1, bytes: 2 }, defaultDir: null, at: null,
  });
  assert.equal(parseStorageCache(null), null);
  assert.equal(parseStorageCache("{nope"), null);
  assert.equal(parseStorageCache("3"), null);
  assert.equal(parseStorageCache(JSON.stringify({ usage: { clipsBytes: 1 } })), null);
  assert.equal(parseStorageCache(JSON.stringify({ usage: { ...usage, cacheBytes: -1 } })), null);
  assert.equal(parseStorageCache(JSON.stringify({ trims: { count: "a", bytes: 1 } })), null);
});

test("bakeCardShown: unknown or positive shows, known zero hides", () => {
  assert.equal(bakeCardShown(null), true);
  assert.equal(bakeCardShown(5), true);
  assert.equal(bakeCardShown(0), false);
});

test("cameraMissing: only after the list was read", () => {
  assert.equal(cameraMissing("a", [], false), false);
  assert.equal(cameraMissing("a", [], true), true);
  assert.equal(cameraMissing("a", ["a"], true), false);
  assert.equal(cameraMissing(null, [], true), false);
});

test("readStillCurrent: discarded once anything invalidated after the start", () => {
  assert.equal(readStillCurrent(3, 3), true);
  assert.equal(readStillCurrent(3, 4), false);
});
