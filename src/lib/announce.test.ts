import { test } from "node:test";
import assert from "node:assert/strict";
import { announce } from "./announce.ts";

test("new text stands as it is", () => {
  assert.equal(announce("", "Moved to position 2 of 4"), "Moved to position 2 of 4");
  assert.equal(announce("Moved to position 1 of 4", "Moved to position 2 of 4"), "Moved to position 2 of 4");
});

test("the same text again changes the region, every time", () => {
  const first = announce("", "Moved to position 2 of 4");
  const second = announce(first, "Moved to position 2 of 4");
  const third = announce(second, "Moved to position 2 of 4");
  assert.notEqual(second, first);
  assert.notEqual(third, second);
  assert.equal(second.trim(), first);
});
