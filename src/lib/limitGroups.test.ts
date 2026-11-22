import { test } from "node:test";
import assert from "node:assert/strict";
import { limitGroups } from "./limitGroups.ts";

const groups = [
  { key: "a", items: [1, 2, 3] },
  { key: "b", items: [4, 5] },
  { key: "c", items: [6] },
];

test("limitGroups cuts across groups and drops the empty ones", () => {
  assert.deepEqual(limitGroups(groups, 4), [
    { key: "a", items: [1, 2, 3] },
    { key: "b", items: [4] },
  ]);
  assert.deepEqual(limitGroups(groups, 3), [{ key: "a", items: [1, 2, 3] }]);
});

test("limitGroups hands the list back untouched without a limit", () => {
  assert.equal(limitGroups(groups, Infinity), groups);
  assert.deepEqual(limitGroups(groups, 99), groups);
});

test("limitGroups gives nothing for a limit of zero", () => {
  assert.deepEqual(limitGroups(groups, 0), []);
});
