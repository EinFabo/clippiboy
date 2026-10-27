import { test } from "node:test";
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { carryOver, DEPARTED_KEEP_MS, departedCutoff, fingerprint } from "./departed.ts";

const DAY = 24 * 60 * 60 * 1000;
const now = 1_800_000_000_000;

test("the fingerprint is the pepper's HMAC-SHA256 of the Discord id, base64url", async () => {
  const expected = createHmac("sha256", "pepper").update("123456789012345678").digest("base64url");
  assert.equal(await fingerprint("pepper", "123456789012345678"), expected);
});

test("another pepper or another id gives another fingerprint", async () => {
  const base = await fingerprint("pepper", "1");
  assert.notEqual(await fingerprint("other", "1"), base);
  assert.notEqual(await fingerprint("pepper", "2"), base);
});

test("without a pepper there is no fingerprint", async () => {
  assert.equal(await fingerprint("", "1"), null);
  assert.equal(await fingerprint(undefined, "1"), null);
});

test("a row is kept for seven days, then the cron may take it", () => {
  assert.equal(DEPARTED_KEEP_MS, 7 * DAY);
  assert.equal(departedCutoff(now), now - 7 * DAY);
});

test("no row, nothing to carry over", () => {
  assert.equal(carryOver(null, now), null);
});

test("a fresh row carries its link times and its suspension", () => {
  const row = { link_times: JSON.stringify([now - 2 * DAY, now - DAY]), banned_at: now - 3 * DAY, left_at: now - DAY };
  assert.deepEqual(carryOver(row, now), { linkTimes: [now - 2 * DAY, now - DAY], bannedAt: now - 3 * DAY });
});

test("a row seven days old carries nothing, even before the cron removed it", () => {
  const row = { link_times: JSON.stringify([now - 7 * DAY + 1]), banned_at: now - 8 * DAY, left_at: now - 7 * DAY };
  assert.equal(carryOver(row, now), null);
  assert.notEqual(carryOver({ ...row, left_at: now - 7 * DAY + 1 }, now), null);
});

test("link times past the week, in the future or not numbers are dropped", () => {
  const row = {
    link_times: JSON.stringify([now - 8 * DAY, now - 7 * DAY, now - DAY, now + DAY, "x", null, now - 2 * DAY]),
    banned_at: null,
    left_at: now - DAY,
  };
  assert.deepEqual(carryOver(row, now), { linkTimes: [now - 2 * DAY, now - DAY], bannedAt: null });
});

test("a row with only stale links and no suspension carries nothing", () => {
  const row = { link_times: JSON.stringify([now - 8 * DAY]), banned_at: null, left_at: now - DAY };
  assert.equal(carryOver(row, now), null);
});

test("an unreadable or missing list still carries the suspension", () => {
  const banned = now - 2 * DAY;
  assert.deepEqual(carryOver({ link_times: "{oops", banned_at: banned, left_at: now - DAY }, now), {
    linkTimes: [],
    bannedAt: banned,
  });
  assert.deepEqual(carryOver({ link_times: null, banned_at: banned, left_at: now - DAY }, now), {
    linkTimes: [],
    bannedAt: banned,
  });
  assert.deepEqual(carryOver({ link_times: '{"a":1}', banned_at: banned, left_at: now - DAY }, now), {
    linkTimes: [],
    bannedAt: banned,
  });
});

test("at most the newest few link times come back, oldest first", () => {
  const times = Array.from({ length: 50 }, (_, i) => now - DAY + i);
  const carried = carryOver({ link_times: JSON.stringify(times), banned_at: null, left_at: now - DAY }, now);
  assert.deepEqual(carried?.linkTimes, times.slice(-10));
});
