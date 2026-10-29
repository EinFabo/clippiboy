import { test } from "node:test";
import assert from "node:assert/strict";
import { driftAction, inGap, slaveTime, steer } from "./slaved.ts";

test("slave time adds the lead", () => {
  assert.equal(slaveTime(3.0, -40), 2.96);
  assert.equal(slaveTime(0.01, -40), 0, "never before the start");
});
test("a small video drift nudges the rate", () => {
  const ahead = driftAction(0.05, "video");
  assert.equal(ahead.seek, false);
  assert.ok(ahead.rate < 1 && ahead.rate >= 0.97, `${ahead.rate}`);
  const behind = driftAction(-0.05, "video");
  assert.ok(behind.rate > 1 && behind.rate <= 1.03, `${behind.rate}`);
});
test("within 10 ms the rate stays even", () => assert.deepEqual(driftAction(0.008, "video"), { seek: false, rate: 1 }));
test("20 ms off is pulled back: two slaves at the edge of one frame sit two frames apart", () =>
  assert.notEqual(driftAction(0.02, "video").rate, 1));
test("a large video drift seeks", () => assert.equal(driftAction(0.4, "video").seek, true));
test("audio seeks past 0.12 s, as the stems always did", () => {
  assert.equal(driftAction(0.13, "audio").seek, true);
  assert.deepEqual(driftAction(0.1, "audio"), { seek: false, rate: 1 });
});

test("steering waits for 25 ms before it nudges, and lets go below 8 ms", () => {
  assert.equal(steer(0.02, false), 1, "not yet");
  assert.ok(steer(0.03, false) < 1, "ahead: slower");
  assert.ok(steer(-0.03, false) > 1, "behind: faster");
  assert.ok(steer(0.015, true) < 1, "still on its way back");
  assert.equal(steer(0.005, true), 1, "back");
});
test("steering never goes past 3 %", () => assert.equal(steer(0.2, true), 0.97));

test("the cam hides inside a gap, in the clean picture's time", () => {
  const gaps: [number, number][] = [[19287, 21996]];
  assert.equal(inGap(19.0, 0, gaps), false);
  assert.equal(inGap(19.5, 0, gaps), true);
  assert.equal(inGap(14.5, 5000, gaps), true, "a clip cut 5 s in: clean time = clip time + lead");
  assert.equal(inGap(22.1, 0, gaps), false);
});
