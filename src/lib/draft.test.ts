import { test } from "node:test";
import assert from "node:assert/strict";
import { labelToSave } from "./draft.ts";

test("a field that was never edited saves nothing", () =>
  assert.equal(labelToSave(null, "Game"), null));
test("an unchanged name saves nothing", () => {
  assert.equal(labelToSave("Game", "Game"), null);
  assert.equal(labelToSave("  Game ", "Game"), null);
});
test("an emptied name is not saved", () => assert.equal(labelToSave("  ", "Game"), null));
test("a new name is saved trimmed", () => assert.equal(labelToSave(" Mic ", "Game"), "Mic"));
