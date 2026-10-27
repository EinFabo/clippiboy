import { test } from "node:test";
import assert from "node:assert/strict";
import { bitrateFor, planExport } from "./exportPlan.ts";

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
