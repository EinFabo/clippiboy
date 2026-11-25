import { test } from "node:test";
import assert from "node:assert/strict";
import {
  canUpload,
  chunks,
  countsAgainstWeek,
  FREE_FAILS,
  givenUp,
  heldClipGoes,
  HELD_MS,
  MAX_OPEN_REPORTS,
  posterRefusal,
  takesReport,
  cleanReport,
  cleanTags,
  expiresIn,
  friendOffer,
  ID_PATTERN,
  looksLikeJpeg,
  Mp4Boxes,
  TOP_LEVEL_BOXES,
  MAX_BYTES,
  newId,
  offersFriend,
  parseRange,
  PENDING_MS,
  quota,
  TOTAL_BYTES,
  WEEK_MS,
  weekStart,
  weekTimes,
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

test("a single range is served as a part, clamped to the file", () => {
  assert.deepEqual(parseRange("bytes=100-", 1000), { kind: "part", offset: 100, length: 900 });
  assert.deepEqual(parseRange("bytes=0-9", 1000), { kind: "part", offset: 0, length: 10 });
  assert.deepEqual(parseRange("bytes=-50", 1000), { kind: "part", offset: 950, length: 50 });
  assert.deepEqual(parseRange("bytes=-5000", 1000), { kind: "part", offset: 0, length: 1000 });
  assert.deepEqual(parseRange("bytes=990-5000", 1000), { kind: "part", offset: 990, length: 10 });
  assert.deepEqual(parseRange("bytes=999-999", 1000), { kind: "part", offset: 999, length: 1 });
});

test("no range, several or a malformed one get the whole file", () => {
  assert.deepEqual(parseRange(null, 1000), { kind: "whole" });
  assert.deepEqual(parseRange("", 1000), { kind: "whole" });
  assert.deepEqual(parseRange("bytes=0-1,5-9", 1000), { kind: "whole" });
  assert.deepEqual(parseRange("bytes=-", 1000), { kind: "whole" });
  assert.deepEqual(parseRange("bytes=9-5", 1000), { kind: "whole" });
  assert.deepEqual(parseRange("items=0-5", 1000), { kind: "whole" });
  assert.deepEqual(parseRange("bytes=a-b", 1000), { kind: "whole" });
});

test("a range past the end cannot be served", () => {
  assert.deepEqual(parseRange("bytes=1000-", 1000), { kind: "unsatisfiable" });
  assert.deepEqual(parseRange("bytes=5000-6000", 1000), { kind: "unsatisfiable" });
  assert.deepEqual(parseRange("bytes=-0", 1000), { kind: "unsatisfiable" });
  assert.deepEqual(parseRange("bytes=99999999999999999999-", 1000), { kind: "unsatisfiable" });
});

test("keys go to R2 in runs it accepts", () => {
  assert.deepEqual(chunks([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
  assert.deepEqual(chunks([], 400), []);
  const keys = Array.from({ length: 1201 }, (_, i) => `k${i}`);
  const runs = chunks(keys, 800);
  assert.equal(runs.length, 2);
  assert.ok(runs.every((run) => run.length <= 1000));
  assert.equal(runs.flat().length, 1201);
});

test("a row is in the running while it is a link or an upload that could still finish", () => {
  // A link, deleted or not, or a failed upload kept as a deleted row.
  assert.equal(countsAgainstWeek({ pending: 0, created_at: now - 3 * 24 * 60 * 60 * 1000 }, now), true);
  // Still on its way.
  assert.equal(countsAgainstWeek({ pending: 1, created_at: now - 60_000 }, now), true);
  // Broke off without a word: stops counting, and the sweep takes it.
  assert.equal(countsAgainstWeek({ pending: 1, created_at: now - PENDING_MS - 1 }, now), false);
});

const hour = 60 * 60 * 1000;
const link = (ago: number) => ({ created_at: now - ago * hour, failed: 0 });
const fail = (ago: number) => ({ created_at: now - ago * hour, failed: 1 });

test("a few failed uploads a week are free; the links alone reach the limit", () => {
  const rows = [fail(9), fail(8), fail(7), link(6), link(5), link(4)];
  const times = weekTimes(rows);
  assert.deepEqual([...times].sort((a, b) => a - b), [now - 6 * hour, now - 5 * hour, now - 4 * hour]);
  assert.deepEqual(canUpload(times, 1, 0), { ok: false, reason: "quota", resetsAt: now - 6 * hour + WEEK_MS });
  // Two links and the same failures: one link left.
  assert.equal(canUpload(weekTimes(rows.slice(0, 5)), 1, 0).ok, true);
  assert.equal(FREE_FAILS, 5);
});

test("past the free ones, failed uploads count — the oldest first", () => {
  const rows = Array.from({ length: 8 }, (_, i) => fail(10 - i));
  const times = weekTimes(rows);
  assert.equal(times.length, 3);
  assert.deepEqual(times, [now - 10 * hour, now - 9 * hour, now - 8 * hour]);
  assert.equal(quota(times, 0).used, 3);
  // The oldest one leaving the week frees the slot.
  assert.equal(quota(times, 0).resetsAt, now - 10 * hour + WEEK_MS);
  assert.deepEqual(weekTimes(rows.slice(0, FREE_FAILS)), []);
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

// The ftyp ffmpeg's mp4 muxer writes: isom, minor version 512, isom iso2 avc1 mp41.
const ffmpegFtyp = box("ftyp", [..."isom"].map((c) => c.charCodeAt(0)).concat([0, 0, 2, 0], [..."isomiso2avc1mp41"].map((c) => c.charCodeAt(0))));
const mdat = box("mdat", new Array(64).fill(0x42));

test("a clip as the app's ffmpeg writes it passes, with its index in front or last", () => {
  // `-movflags +faststart`, as every link copy is made: ftyp, moov, the free
  // box kept for a 64-bit mdat size, mdat.
  assert.equal(walk([...ffmpegFtyp, ...box("moov", [1, 2, 3, 4]), ...box("free"), ...mdat], 7), true);
  // Without it, moov comes last.
  assert.equal(walk([...ffmpegFtyp, ...box("free"), ...mdat, ...box("moov", [1, 2])], 5), true);
});

test("a fragmented MP4 passes", () => {
  const fragment = [...box("moof", [9, 9, 9]), ...box("mdat", [1, 2, 3, 4])];
  const file = [...ffmpegFtyp, ...box("moov", [1]), ...box("sidx", [0, 0]), ...fragment, ...fragment, ...box("mfra", [5])];
  assert.equal(walk(file, 1), true);
  assert.equal(walk(file, 4096), true);
});

test("a box that runs to the end of the file lets nothing through behind a moov", () => {
  // The bypass: ftyp, then a moov of size 0, then anything at all.
  const anything = [..."<html><script>alert(1)</script>"].map((c) => c.charCodeAt(0));
  assert.equal(walk([...ftyp, 0, 0, 0, 0, ...[..."moov"].map((c) => c.charCodeAt(0)), ...anything]), false);
  // A last mdat of size 0 is refused just the same.
  assert.equal(walk([...ffmpegFtyp, ...box("moov", [1]), 0, 0, 0, 0, ...[..."mdat"].map((c) => c.charCodeAt(0)), 1, 2, 3]), false);
});

test("a box of a kind no MP4 has at its top, or a second ftyp, fails", () => {
  assert.equal(walk([...ftyp, ...box("moov"), ...box("zzzz", [1, 2]), ...mdat]), false);
  assert.equal(walk([...ftyp, ...box("moov"), ...box("html", [1, 2])]), false);
  assert.equal(walk([...ftyp, ...box("moov"), ...ftyp, ...mdat]), false);
  // A box shorter than its own header.
  assert.equal(walk([...ftyp, ...box("moov"), 0, 0, 0, 4, ...[..."free"].map((c) => c.charCodeAt(0))]), false);
  // And every name the list keeps is one an MP4 may have.
  for (const type of TOP_LEVEL_BOXES) {
    if (type === "ftyp") continue;
    assert.equal(walk([...ftyp, ...box("moov"), ...box(type, [0, 0, 0, 0])]), true, type);
  }
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

test("only an explicit friend=0 turns the friend button off", () => {
  assert.equal(friendOffer("0"), 0);
  assert.equal(friendOffer(null), 1);
  assert.equal(friendOffer("1"), 1);
  assert.equal(friendOffer(""), 1);
});

test("the friend button needs the name, the link's offer and open requests", () => {
  assert.equal(offersFriend({ show_name: 1, offer_friend: 1 }, 1), true);
  assert.equal(offersFriend({ show_name: 1, offer_friend: 0 }, 1), false);
  assert.equal(offersFriend({ show_name: 0, offer_friend: 1 }, 1), false);
  assert.equal(offersFriend({ show_name: 1, offer_friend: 1 }, 0), false);
});

test("an upload that broke off without a word counts like a failed one once the sweep gives it up", () => {
  const start = 1_000_000_000;
  const now = start + PENDING_MS + 1;
  const pending = { pending: 1, failed: 0, bytes: MAX_BYTES, created_at: start, deleted_at: null };
  // Stale and still pending, it no longer counts — the sweep must not leave it so.
  assert.equal(countsAgainstWeek(pending, now), false);
  const gone = givenUp(pending, now);
  assert.deepEqual(gone, { pending: 0, failed: 1, bytes: 0, created_at: start, deleted_at: now });
  assert.equal(countsAgainstWeek(gone, now), true);
  // Past the free ones, each takes a link.
  const rows = Array.from({ length: FREE_FAILS + 2 }, (_, i) => givenUp({ ...pending, created_at: start + i }, now));
  assert.deepEqual(weekTimes(rows), [start, start + 1]);
});

test("giving up leaves a finished link alone", () => {
  const link = { pending: 0, failed: 0, bytes: 10, created_at: 1, deleted_at: null };
  assert.equal(givenUp(link, 5), link);
});

test("a poster goes only on a finished link that is still up", () => {
  assert.equal(posterRefusal({ pending: 0, deleted_at: null }), null);
  assert.equal(posterRefusal({ pending: 1, deleted_at: null }), "uploading");
  assert.equal(posterRefusal({ pending: 0, deleted_at: 5 }), "gone");
});

test("one address reports a clip once, however often it sends", () => {
  const open = [{ reporter: "a" }];
  assert.equal(takesReport(open, "a"), false);
  assert.equal(takesReport(open, "b"), true);
});

test("the cap counts addresses, and reports from before the fingerprint count for nobody", () => {
  const full = Array.from({ length: MAX_OPEN_REPORTS }, (_, i) => ({ reporter: `r${i}` }));
  assert.equal(takesReport(full, "new"), false);
  const legacy = [...full.slice(1), ...Array.from({ length: 30 }, () => ({ reporter: null }))];
  assert.equal(takesReport(legacy, "new"), true);
});

test("a held clip stays while a report is open, and never past its time", () => {
  const now = 10 * HELD_MS;
  assert.equal(heldClipGoes(now - 1000, 2, now), false);
  assert.equal(heldClipGoes(now - 1000, 0, now), true);
  assert.equal(heldClipGoes(now - HELD_MS, 2, now), true);
  assert.equal(heldClipGoes(now - HELD_MS + 1, 1, now), false);
});
