import { test } from "node:test";
import assert from "node:assert/strict";
import { groupKey, readOpen, writeOpen } from "./groupState.ts";

const store = () => {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), m };
};
const broken = {
  getItem: () => {
    throw new Error("blocked");
  },
  setItem: () => {
    throw new Error("blocked");
  },
};

test("key is per group", () => assert.equal(groupKey("banner"), "cb.settings.group.banner"));

test("falls back to the default and round-trips", () => {
  const s = store();
  assert.equal(readOpen("a", false, s), false);
  assert.equal(readOpen("a", true, s), true);
  writeOpen("a", true, s);
  assert.equal(readOpen("a", false, s), true);
  writeOpen("a", false, s);
  assert.equal(readOpen("a", true, s), false);
});

test("a throwing storage falls back and does not throw on write", () => {
  assert.equal(readOpen("a", true, broken), true);
  writeOpen("a", true, broken);
});
