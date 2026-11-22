import { test } from "node:test";
import assert from "node:assert/strict";
import { formatAgo } from "./format.ts";

const MIN = 60_000;
const NOW = Date.UTC(2026, 10, 20, 15, 30);

test("the last minute is just now", () => {
  assert.equal(formatAgo(NOW, NOW), "just now");
  assert.equal(formatAgo(NOW - 20_000, NOW), "just now");
});
test("minutes, then hours", () => {
  assert.equal(formatAgo(NOW - 5 * MIN, NOW), "5m ago");
  assert.equal(formatAgo(NOW - 59 * MIN, NOW), "59m ago");
  assert.equal(formatAgo(NOW - 3 * 60 * MIN, NOW), "3h ago");
});
test("older than a day reads exactly as toLocaleDateString did", () => {
  for (const days of [1, 2, 30, 200, 400, 2000]) {
    const ts = NOW - days * 24 * 60 * MIN - 7 * MIN;
    const old = new Date(ts).toLocaleDateString("en-US", { day: "numeric", month: "long", year: "numeric" });
    assert.equal(formatAgo(ts, NOW), old);
  }
});
test("without a clock it uses the current time", () => {
  assert.equal(formatAgo(Date.now() - 2 * MIN), "2m ago");
});
