import { test } from "node:test";
import assert from "node:assert/strict";
import { later, takes } from "./revision.ts";

const at = (revision: number, name = "") => ({ revision, name });

test("a config from after the one held replaces it", () =>
  assert.equal(takes(at(4), at(5)), true));
test("an answer that left the core before the copy held is older news", () =>
  assert.equal(takes(at(6), at(5)), false));
test("the same revision again is no news", () =>
  assert.equal(takes(at(5), at(5)), false));
test("a fresh read stands in for the optimistic copy on its own revision", () => {
  assert.equal(takes(at(5), at(5), true), true);
  assert.equal(takes(at(6), at(5), true), false);
});

test("of an event held back and another, the later one is kept", () => {
  assert.deepEqual(later(null, at(3, "a")), at(3, "a"));
  assert.deepEqual(later(at(3, "a"), at(5, "b")), at(5, "b"));
  assert.deepEqual(later(at(5, "b"), at(4, "c")), at(5, "b"));
});
