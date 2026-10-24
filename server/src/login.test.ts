import { test } from "node:test";
import assert from "node:assert/strict";
import { statesIn, withState } from "./login.ts";

test("a second sign-in start keeps the first tab's state", () => {
  const first = withState(statesIn("x=1", "cb_login"), "aaa");
  const both = withState(statesIn(`x=1; cb_login=${first}`, "cb_login"), "bbb");
  assert.deepEqual(statesIn(`cb_login=${both}`, "cb_login"), ["aaa", "bbb"]);
});
test("only the last three starts are kept", () => assert.equal(withState(["a", "b", "c"], "d"), "b.c.d"));
test("no cookie, or someone else's, holds no state", () => {
  assert.deepEqual(statesIn(null, "cb_login"), []);
  assert.deepEqual(statesIn("other=abc", "cb_login"), []);
});
