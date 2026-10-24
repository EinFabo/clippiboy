import { test } from "node:test";
import assert from "node:assert/strict";
import { beyondCap, CONTACT_MAX, CONTACT_MS, mayReach, remember, spend, type Contacts } from "./hubrules.ts";

const now = 1_000_000;

test("an invisible user can be reached only by friends they wrote to in the last ten minutes", () => {
  const contacts = remember({}, "f1", now);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS - 1), true);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS), false);
  assert.equal(mayReach(contacts, "f2", now), false);
  assert.equal(mayReach(undefined, "f1", now), false);
});

test("the list drops what expired and keeps the newest fifty", () => {
  let contacts: Contacts = { old: now - 1 };
  for (let i = 0; i < 60; i++) contacts = remember(contacts, `f${i}`, now + i);
  assert.equal(Object.keys(contacts).length, CONTACT_MAX);
  assert.equal("old" in contacts || "f0" in contacts, false);
  assert.equal("f59" in contacts, true);
});

const rule = { capacity: 3, refillMs: 1000 };

test("a fresh budget lets a burst through, then says when the next one may go", () => {
  let b = spend(undefined, rule, now);
  b = spend(spend(b.bucket, rule, now).bucket, rule, now);
  assert.equal(b.ok, true);
  const fourth = spend(b.bucket, rule, now);
  assert.deepEqual([fourth.ok, fourth.nextAt], [false, now + 1000]);
});

test("a budget fills up with time, never above its capacity", () => {
  let bucket = spend(undefined, rule, now).bucket;
  const later = now + 60_000;
  const oks = [1, 2, 3, 4].map(() => { const r = spend(bucket, rule, later); bucket = r.bucket; return r.ok; });
  assert.deepEqual(oks, [true, true, true, false]);
});

test("the newest four connections stay, the oldest goes", () => {
  const sockets = [5, 1, 3, 4, 2].map((opened) => ({ opened }));
  assert.deepEqual(beyondCap(sockets, 4), [{ opened: 1 }]);
  assert.deepEqual(beyondCap([{ opened: 9 }, {}], 1), [{}]);
  assert.deepEqual(beyondCap(sockets.slice(0, 3), 4), []);
});

test("a budget restored from storage keeps throttling", () => {
  let bucket = spend(undefined, rule, now).bucket;
  for (let i = 0; i < 2; i++) bucket = spend(bucket, rule, now).bucket;
  const restored = JSON.parse(JSON.stringify(bucket));
  assert.equal(spend(restored, rule, now + 500).ok, false);
  assert.equal(spend(restored, rule, now + 1000).ok, true);
});
