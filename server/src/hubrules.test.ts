import { test } from "node:test";
import assert from "node:assert/strict";
import { beyondCap, CONTACT_MAX, CONTACT_MS, hidesNow, MESSAGES, mayReach, pruned, RELAY, remember, spend, type Bucket, type Contacts } from "./hubrules.ts";

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

test("pruning drops expired contacts and reports an empty list as nothing", () => {
  const contacts = { old: now - 1, fresh: now + 5 };
  assert.deepEqual(pruned(contacts, now), { fresh: now + 5 });
  assert.equal(pruned({ old: now }, now), undefined);
  assert.equal(pruned(undefined, now), undefined);
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

test("turning Appear offline on is published at once, past the throttle", () => {
  const playing = { game: "Valorant" };
  assert.equal(hidesNow(playing, null), true);
});

test("other changes wait for the presence budget", () => {
  assert.equal(hidesNow(null, { game: "Valorant" }), false);
  assert.equal(hidesNow({ game: null }, { game: "Valorant" }), false);
  assert.equal(hidesNow(null, null), false);
});

test("a refused spend need not be stored: the old bucket refills to the same", () => {
  const low: Bucket = { tokens: 0.2, at: now };
  const refused = spend(low, rule, now + 100);
  assert.equal(refused.ok, false);
  for (const later of [now + 500, now + 800, now + 900, now + 5_000]) {
    const fromOld = spend(low, rule, later);
    const fromRefused = spend(refused.bucket, rule, later);
    assert.equal(fromOld.ok, fromRefused.ok);
    assert.ok(Math.abs(fromOld.bucket.tokens - fromRefused.bucket.tokens) < 1e-9);
  }
});

test("a flood of messages is cut off after twenty, and a steady two a second gets through", () => {
  let bucket: Bucket | undefined;
  let passed = 0;
  for (let i = 0; i < 100; i++) {
    const r = spend(bucket, MESSAGES, now);
    bucket = r.bucket;
    if (r.ok) passed++;
  }
  assert.equal(passed, 20);
  let steady: Bucket | undefined = { tokens: 0, at: now };
  for (let t = now + 500; t <= now + 60_000; t += 500) {
    const r = spend(steady, MESSAGES, t);
    assert.equal(r.ok, true);
    steady = r.bucket;
  }
});

test("relays: thirty at once, then no more than ten a minute", () => {
  let bucket: Bucket | undefined;
  let passed = 0;
  // Two a second for an hour, the old budget's pace and more.
  for (let t = now; t < now + 3_600_000; t += 500) {
    const r = spend(bucket, RELAY, t);
    bucket = r.bucket;
    if (r.ok) passed++;
  }
  // The burst, then one every 6 s; the last of the hour lands just past it.
  assert.ok(passed >= 30 + 599 && passed <= 30 + 600, `${passed}`);
});
