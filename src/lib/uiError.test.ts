import { test } from "node:test";
import assert from "node:assert/strict";
import { describeError, UI_ERROR_MAX } from "./uiError.ts";

test("an Error gives its stack, a component stack is appended", () => {
  const text = describeError(new Error("boom"), "\n    at Dashboard");
  assert.match(text, /boom/);
  assert.match(text, /at Dashboard/);
});

test("anything else is stringified", () => {
  assert.equal(describeError("plain"), "plain");
  assert.equal(describeError({ toString() { throw 1; } }), "unprintable error");
});

test("a huge text is cut", () => {
  assert.equal(describeError("x".repeat(UI_ERROR_MAX * 5)).length, UI_ERROR_MAX);
});
