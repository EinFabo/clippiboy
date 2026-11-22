import { test } from "node:test";
import assert from "node:assert/strict";
import { isDuplicateToast } from "./toastDedupe.ts";

const last = { kind: "error", message: "Too long", at: 1000 };

test("same kind and message within 1.5 s is a duplicate", () => {
  assert.equal(isDuplicateToast(last, { kind: "error", message: "Too long" }, 2499), true);
});
test("later, another message or another kind is not", () => {
  assert.equal(isDuplicateToast(last, { kind: "error", message: "Too long" }, 2500), false);
  assert.equal(isDuplicateToast(last, { kind: "error", message: "Other" }, 1100), false);
  assert.equal(isDuplicateToast(last, { kind: "ok", message: "Too long" }, 1100), false);
  assert.equal(isDuplicateToast(null, { kind: "ok", message: "x" }, 1100), false);
});
