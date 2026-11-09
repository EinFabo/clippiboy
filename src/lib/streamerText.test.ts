import { test } from "node:test";
import assert from "node:assert/strict";
import { HIDDEN_NAME, shownCode, shownName } from "./streamerText.ts";

test("streamer mode puts a friend in place of the name", () => {
  assert.equal(shownName("Luca", false), "Luca");
  assert.equal(shownName("Luca", true), HIDDEN_NAME);
});

test("streamer mode shows nothing of the friend code", () => {
  assert.equal(shownCode("ABCD-EFGH", false), "ABCD-EFGH");
  assert.ok(!shownCode("ABCD-EFGH", true).includes("ABCD"));
});
