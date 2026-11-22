import { test } from "node:test";
import assert from "node:assert/strict";
import { decayLevel, levelPct, riseLevel } from "./meter.ts";

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
