import { test } from "node:test";
import assert from "node:assert/strict";
import {
  decayLevel,
  GAIN_SETTLE_MS,
  holdGain,
  HOT_HOLD_MS,
  HOT_LEVEL,
  holdPeak,
  hotUntil,
  levelPct,
  meterGain,
  PEAK_HOLD_MS,
  peakDbText,
  preGain,
  riseLevel,
  simulatedLevel,
} from "./meter.ts";

test("one 60 ms step keeps 78 percent", () => assert.ok(Math.abs(decayLevel(1, 60) - 0.78) < 1e-9));
test("decay over elapsed time composes", () => {
  const split = decayLevel(decayLevel(0.8, 16), 44);
  assert.ok(Math.abs(split - decayLevel(0.8, 60)) < 1e-9);
});
test("zero elapsed time changes nothing", () => assert.equal(decayLevel(0.5, 0), 0.5));
test("tiny levels snap to zero", () => {
  assert.equal(decayLevel(0.004, 16), 0);
  assert.equal(decayLevel(0.0055, 60), 0);
});
test("levels rise instantly, never fall by riseLevel", () => {
  assert.equal(riseLevel(0.2, 0.6), 0.6);
  assert.equal(riseLevel(0.6, 0.2), 0.6);
});
test("dB scale: silence 0, full 100, -30 dB half", () => {
  assert.equal(levelPct(0), 0);
  assert.equal(levelPct(1), 100);
  assert.ok(Math.abs(levelPct(Math.pow(10, -30 / 20)) - 50) < 1e-9);
  assert.equal(levelPct(5), 100);
});

test("a hot level starts the hold, a quiet one keeps it", () => {
  assert.equal(hotUntil(0, 0.95, 1000), 1000 + HOT_HOLD_MS);
  assert.equal(hotUntil(0, HOT_LEVEL, 1000), 1000 + HOT_HOLD_MS);
  assert.equal(hotUntil(2200, 0.5, 1500), 2200);
  assert.equal(hotUntil(0, 0.5, 1500), 0);
  // A source turned up past full scale counts as well.
  assert.equal(hotUntil(0, 1.6, 10), 10 + HOT_HOLD_MS);
});
test("a held peak rises at once and falls only after the hold", () => {
  const held = { level: 0.5, at: 0 };
  assert.deepEqual(holdPeak(held, 0.7, 100), { level: 0.7, at: 100 });
  assert.equal(holdPeak(held, 0.2, 500), held);
  assert.deepEqual(holdPeak(held, 0.2, PEAK_HOLD_MS), { level: 0.2, at: PEAK_HOLD_MS });
});
test("peak text: whole dB, Silent at the bottom", () => {
  assert.equal(peakDbText(0), "Silent");
  assert.equal(peakDbText(0.0005), "Silent");
  assert.equal(peakDbText(0.25), "-12 dB");
  assert.equal(peakDbText(1), "0 dB");
  assert.equal(peakDbText(0.97), "0 dB");
  assert.equal(peakDbText(1.26), "+2 dB");
  assert.equal(peakDbText(Number.NaN), "Silent");
});
test("pre-gain: the volume taken back out, capped at full scale", () => {
  assert.ok(Math.abs(preGain(0.5, 0) - 0.5) < 1e-9);
  assert.ok(Math.abs(preGain(10 ** (6 / 20) * 0.25, 6) - 0.25) < 1e-9);
  assert.ok(Math.abs(preGain(0.1, -20) - 1) < 1e-9);
  assert.equal(preGain(2, 0), 1);
  assert.equal(preGain(0, 12), 0);
  assert.equal(preGain(Number.NaN, 0), 0);
});

test("after a big drop the old gain stays in the meter until the core has it", () => {
  // +12 dB down to -30 dB at t=1000: a packet still measured at +12 dB…
  const hold = holdGain(null, 12, 1000);
  const late = 0.5 * 10 ** (12 / 20);
  assert.equal(meterGain(-30, hold, 1100), 12);
  assert.ok(Math.abs(preGain(late, meterGain(-30, hold, 1100)) - 0.5) < 1e-9);
  // …where the new gain alone fills the bar.
  assert.equal(preGain(late, -30), 1);
  // Once settled, the new gain counts.
  assert.equal(meterGain(-30, hold, 1000 + GAIN_SETTLE_MS), -30);
  assert.equal(meterGain(-30, null, 1100), -30);
});

test("a raise counts at once, and a change during a hold keeps the higher gain", () => {
  assert.equal(meterGain(6, holdGain(null, 0, 1000), 1100), 6);
  const first = holdGain(null, 12, 1000);
  const second = holdGain(first, 0, 1200);
  assert.equal(second.db, 12);
  assert.equal(second.until, 1200 + GAIN_SETTLE_MS);
  // A hold that ran out is not carried on.
  assert.equal(holdGain(first, 0, 1000 + GAIN_SETTLE_MS).db, 0);
});

test("browser levels: the volume applied, nothing off, muted or left out by Solo", () => {
  const s = { enabled: true, muted: false, solo: false, gainDb: 0 };
  assert.equal(simulatedLevel(s, false, 0.4), 0.4);
  assert.ok(Math.abs(simulatedLevel({ ...s, gainDb: -20 }, false, 0.4) - 0.04) < 1e-9);
  assert.equal(simulatedLevel({ ...s, muted: true }, false, 0.4), 0);
  assert.equal(simulatedLevel({ ...s, enabled: false }, false, 0.4), 0);
  assert.equal(simulatedLevel(s, true, 0.4), 0);
  assert.equal(simulatedLevel({ ...s, solo: true }, true, 0.4), 0.4);
});
