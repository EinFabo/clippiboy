import { test } from "node:test";
import assert from "node:assert/strict";
import {
  byteRange,
  canUpload,
  cleanReport,
  cleanTags,
  expiresIn,
  ID_PATTERN,
  looksLikeJpeg,
  Mp4Boxes,
  MAX_BYTES,
  newId,
  quota,
  TOTAL_BYTES,
  WEEK_MS,
  weekStart,
} from "./shares.ts";

const MB = 1024 * 1024;
const now = 1_800_000_000_000;

test("a fresh account may upload", () => {
  assert.deepEqual(canUpload([], 30 * MB, 0), { ok: true });
});

test("a clip over 50 MB is turned away before anything else", () => {
  assert.deepEqual(canUpload([now, now, now], MAX_BYTES + 1, 0), { ok: false, reason: "too_big" });
});

test("the fourth link of the week waits until the oldest leaves the week", () => {
  const recent = [now - 3_000, now - 1_000, now - 2_000];
  assert.deepEqual(canUpload(recent, MB, 0), { ok: false, reason: "quota", resetsAt: now - 3_000 + WEEK_MS });
});

test("two of three used still leaves one", () => {
  assert.deepEqual(canUpload([now, now], MB, 0), { ok: true });
});

test("a full bucket says so", () => {
  assert.deepEqual(canUpload([], 10 * MB, TOTAL_BYTES - 5 * MB), { ok: false, reason: "full" });
});

test("a small clip still fits where a large one would not", () => {
  const live = TOTAL_BYTES - 20 * MB;
  assert.deepEqual(canUpload([], 10 * MB, live), { ok: true });
  assert.equal(quota([], live).full, true);
});

test("the quota reports what is used and when it frees up", () => {
  assert.deepEqual(quota([], 0), { used: 0, limit: 3, resetsAt: null, full: false });
  assert.deepEqual(quota([now - 10, now - 20], 0), { used: 2, limit: 3, resetsAt: now - 20 + WEEK_MS, full: false });
});

test("ids are ten unambiguous characters", () => {
  for (let i = 0; i < 200; i++) assert.match(newId(), ID_PATTERN);
  assert.doesNotMatch("abcdefghi1", ID_PATTERN);
  assert.doesNotMatch("../etc/pas", ID_PATTERN);
});

test("the page says how long is left", () => {
  const day = 24 * 60 * 60 * 1000;
  assert.equal(expiresIn(now + 5 * day, now), "in 5 days");
  assert.equal(expiresIn(now + 5 * day - 60_000, now), "in 5 days");
  assert.equal(expiresIn(now + day + 5, now), "in 1 day");
  assert.equal(expiresIn(now + 3 * 60 * 60 * 1000, now), "in 3 hours");
  assert.equal(expiresIn(now + 60_000, now), "in a few minutes");
});

test("ranges come back as offset and length", () => {
  assert.deepEqual(byteRange({ offset: 100 }, 1000), { offset: 100, length: 900 });
  assert.deepEqual(byteRange({ offset: 0, length: 10 }, 1000), { offset: 0, length: 10 });
  assert.deepEqual(byteRange({ suffix: 50 }, 1000), { offset: 950, length: 50 });
});

test("tags are trimmed, unique and capped", () => {
  assert.deepEqual(cleanTags([" ace ", "Ace", "", "clutch"]), ["ace", "clutch"]);
  assert.equal(cleanTags(Array.from({ length: 30 }, (_, i) => `t${i}`)).length, 12);
  assert.equal(cleanTags(["x".repeat(100)])[0].length, 40);
});

const box = (type: string, body: number[] = []) => {
  const size = 8 + body.length;
  return [size >>> 24, (size >> 16) & 255, (size >> 8) & 255, size & 255, ...Array.from(type, (c) => c.charCodeAt(0)), ...body];
};
const walk = (bytes: number[], chunk = 3) => {
  const boxes = new Mp4Boxes();
  for (let i = 0; i < bytes.length; i += chunk) if (!boxes.push(new Uint8Array(bytes.slice(i, i + chunk)))) return false;
  return boxes.end();
};
const ftyp = box("ftyp", Array.from("isom\0\0\0\0isommp41", (c) => c.charCodeAt(0)));

test("a whole MP4 passes, in any chunking", () => {
  const file = [...ftyp, ...box("moov", [1, 2, 3]), ...box("mdat", new Array(50).fill(7))];
  assert.equal(walk(file, 1), true);
  assert.equal(walk(file, 1000), true);
});

test("an ftyp glued in front of something else fails", () => {
  const zip = [0x50, 0x4b, 3, 4, ...new Array(40).fill(0)];
  assert.equal(walk([0, 0, 0, 8, 0x66, 0x74, 0x79, 0x70, ...zip]), false);
  assert.equal(walk([...ftyp, ...zip]), false);
});

test("an MP4 with something hung on behind, or without moov, fails", () => {
  assert.equal(walk([...ftyp, ...box("moov"), ...box("mdat", [1]), 0x50, 0x4b]), false);
  assert.equal(walk([...ftyp, ...box("mdat", [1, 2])]), false);
  assert.equal(walk([0x4d, 0x5a, 0x90, 0, 3, 0, 0, 0]), false);
});

test("only a JPEG passes as a poster", () => {
  assert.equal(looksLikeJpeg(new Uint8Array([0xff, 0xd8, 0xff, 0xe0])), true);
  assert.equal(looksLikeJpeg(new Uint8Array([0x89, 0x50, 0x4e, 0x47])), false);
});

test("the week starts seven days back, or at a reset if that came later", () => {
  assert.equal(weekStart(now, 0), now - WEEK_MS);
  assert.equal(weekStart(now, now - 1_000), now - 1_000);
  assert.equal(weekStart(now, now - 2 * WEEK_MS), now - WEEK_MS);
});

test("a report needs a known reason and keeps a short note at most", () => {
  assert.deepEqual(cleanReport({ reason: "spam" }), { reason: "spam", note: null });
  assert.deepEqual(cleanReport({ reason: "other", note: "  stolen clip  " }), { reason: "other", note: "stolen clip" });
  assert.equal(cleanReport({ reason: "other", note: "x".repeat(500) })!.note!.length, 300);
  assert.deepEqual(cleanReport({ reason: "nsfw", note: "   " }), { reason: "nsfw", note: null });
  assert.equal(cleanReport({ reason: "boring" }), null);
  assert.equal(cleanReport({ reason: 5 }), null);
  assert.equal(cleanReport(null), null);
});
