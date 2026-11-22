import { test } from "node:test";
import assert from "node:assert/strict";
import {
  bufferPreset,
  bufferPresetLabel,
  clipLength,
  clipMegabytes,
  megabytesPerMinute,
  memoryBudgetMb,
  memoryLimitSummary,
} from "./videoPlan.ts";

test("buffer presets match only the four steps", () => {
  assert.equal(bufferPreset(30), 30);
  assert.equal(bufferPreset(120), 120);
  assert.equal(bufferPreset(300), 300);
  assert.equal(bufferPreset(45), null);
  assert.equal(bufferPreset(600), null);
  assert.deepEqual([30, 60, 120, 300].map(bufferPresetLabel), ["30 s", "1 min", "2 min", "5 min"]);
});

test("megabytes per minute follow the clip maths", () => {
  assert.equal(megabytesPerMinute(20000), clipMegabytes(20000, 60));
  assert.equal(megabytesPerMinute(20000), 148);
});

test("memory budget: explicit wins, else bitrate with half on top, at least 64", () => {
  assert.equal(memoryBudgetMb({ bitrateKbps: 20000 }, { seconds: 120, memoryMb: 512 }), 512);
  assert.equal(memoryBudgetMb({ bitrateKbps: 20000 }, { seconds: 120, memoryMb: 0 }), 439);
  assert.equal(memoryBudgetMb({ bitrateKbps: 1000 }, { seconds: 15, memoryMb: 0 }), 64);
});

test("clip length is capped by the buffer; 0 means all of it", () => {
  assert.equal(clipLength({ seconds: 120, clipSeconds: 0 }), 120);
  assert.equal(clipLength({ seconds: 60, clipSeconds: 90 }), 60);
  assert.equal(clipLength({ seconds: 120, clipSeconds: 30 }), 30);
});

test("memory limit summary: enough, or only so many seconds", () => {
  assert.deepEqual(memoryLimitSummary(512, 20000, 120), { text: "512 MB · enough for the full 2 min", warn: false });
  assert.deepEqual(memoryLimitSummary(220, 20000, 120), { text: "220 MB · only about 1 min at this quality", warn: true });
});
