import { test } from "node:test";
import assert from "node:assert/strict";
import { bitrateFor, planExport, roundMb } from "./exportPlan.ts";

const MB = 1024 * 1024;
test("the bitrate mirror clamps like the core", () => {
  assert.equal(bitrateFor(404, 720, 30), 2000);
  assert.equal(bitrateFor(1920, 1080, 60), 19000);
});
test("a portrait clip on a thin budget goes to 480, as the core does", () => {
  const plan = planExport({ durationMs: 60_000, width: 1080, height: 1920 }, Math.floor(6.1 * MB));
  assert.deepEqual([plan.width, plan.height], [270, 480]);
});
test("a 1440p clip with room keeps its height", () =>
  assert.equal(planExport({ durationMs: 30_000, width: 2560, height: 1440 }, 100 * MB).height, 1440));
test("a 5.9 s clip is budgeted for 5.9 s", () => {
  const plan = planExport({ durationMs: 5_900, width: 1920, height: 1080 }, 10 * MB);
  assert.ok(((plan.videoKbps + 128) * 5_900) / 8 <= 10 * MB);
});
test("a clip too long for its target says so, after the sound gave way", () => {
  const tenMinutes = planExport({ durationMs: 600_000, width: 1920, height: 1080 }, 10 * MB);
  assert.equal(tenMinutes.tooLong, true);
  assert.deepEqual([tenMinutes.videoKbps, tenMinutes.audioKbps], [200, 64]);
  assert.ok(tenMinutes.expectedMb > 19 && tenMinutes.expectedMb < 20, `expected ${tenMinutes.expectedMb} MB`);
  const rescued = planExport({ durationMs: 280_000, width: 1920, height: 1080 }, 10 * MB);
  assert.equal(rescued.tooLong, false);
  assert.equal(rescued.audioKbps, 64);
  const short = planExport({ durationMs: 60_000, width: 1920, height: 1080 }, 10 * MB);
  assert.equal(short.audioKbps, 128);
});
test("sizes round to one decimal", () => {
  assert.equal(roundMb(9.333333), 9.3);
  assert.equal(roundMb(25), 25);
});
