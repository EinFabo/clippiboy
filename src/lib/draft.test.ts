import { test } from "node:test";
import assert from "node:assert/strict";
import { labelToSave, settlesWithoutChange } from "./draft.ts";

test("a field that was never edited saves nothing", () =>
  assert.equal(labelToSave(null, "Game"), null));
test("an unchanged name saves nothing", () => {
  assert.equal(labelToSave("Game", "Game"), null);
  assert.equal(labelToSave("  Game ", "Game"), null);
});
test("an emptied name is not saved", () => assert.equal(labelToSave("  ", "Game"), null));
test("a new name is saved trimmed", () => assert.equal(labelToSave(" Mic ", "Game"), "Mic"));

test("a knob let go where it was pressed leaves a draft no change event clears", () => {
  assert.equal(settlesWithoutChange("10", "10"), true);
  assert.equal(settlesWithoutChange("-3.5", "-3.5"), true);
});
test("a knob let go elsewhere is cleared by the change event", () =>
  assert.equal(settlesWithoutChange("10", "15"), false));
test("a release without a press settles nothing", () =>
  assert.equal(settlesWithoutChange(null, "10"), false));
