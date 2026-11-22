import { test } from "node:test";
import assert from "node:assert/strict";
import {
  audioSummary,
  bufferStatus,
  isJustNow,
  justNowLeft,
  latestItems,
  saveStep,
  videoSummary,
} from "./overview.ts";
import type { Clip } from "./types.ts";

const at = (id: string, createdAt: number) => ({ id, createdAt }) as Clip;

test("a buffer with a game is protecting it", () => {
  const s = bufferStatus({ bufferActive: true, game: "Counter-Strike 2", bufferedSeconds: 102, bufferSeconds: 120 });
  assert.equal(s.state, "protecting");
  assert.equal(s.title, "Buffering Counter-Strike 2");
  assert.equal(s.sub, "1 min 42 s of 2 min ready to save");
  assert.ok(Math.abs(s.fill - 0.85) < 1e-9);
});
test("a buffer without a game waits for one", () => {
  const s = bufferStatus({ bufferActive: true, game: null, bufferedSeconds: 30, bufferSeconds: 120 });
  assert.equal(s.state, "waiting");
  assert.equal(s.title, "Ready · waiting for a game");
  assert.equal(s.sub, "30 s of 2 min ready to save");
});
test("a stopped buffer says nothing is saved, whatever else is set", () => {
  const s = bufferStatus({ bufferActive: false, game: "X", bufferedSeconds: 50, bufferSeconds: 120 });
  assert.equal(s.state, "off");
  assert.equal(s.title, "Buffer is off — nothing is being saved");
  assert.equal(s.fill, 0);
});
test("a stopped buffer with a recording running is not 'nothing saved'", () => {
  const s = bufferStatus({ bufferActive: false, game: null, bufferedSeconds: 0, bufferSeconds: 120, recording: true });
  assert.equal(s.state, "recording");
  assert.equal(s.title, "Buffer is off · recording");
});
test("the fill stays between 0 and 1", () => {
  assert.equal(bufferStatus({ bufferActive: true, game: null, bufferedSeconds: 500, bufferSeconds: 120 }).fill, 1);
  assert.equal(bufferStatus({ bufferActive: true, game: null, bufferedSeconds: 5, bufferSeconds: 0 }).fill, 0);
});

test("the latest are the newest three of any kind", () => {
  const list = [at("a", 1), at("d", 4), at("b", 2), at("c", 3)];
  assert.deepEqual(latestItems(list).map((c) => c.id), ["d", "c", "b"]);
  assert.equal(list[0].id, "a");
});

test("just now is under a minute", () => {
  assert.equal(isJustNow(1000, 1000 + 59_999), true);
  assert.equal(isJustNow(1000, 1000 + 60_000), false);
});
test("just now says how long it has left", () => {
  assert.equal(justNowLeft(0, 20_000), 40_000);
  assert.equal(justNowLeft(0, 60_000), null);
});

test("the save step names the key, or goes without", () => {
  assert.equal(saveStep("F9"), "Press F9");
  assert.equal(saveStep(null), "Save a clip");
});
test("video and audio are summed up in words", () => {
  assert.equal(videoSummary({ height: 1080, fps: 60, quality: 70 }, "NVIDIA NVENC", false), "1080p60 · Balanced · NVIDIA NVENC");
  assert.equal(videoSummary({ height: 1080, fps: 60, quality: 90 }, "x264", true), "1080p60 · Highest · x264 · fixed bitrate");
  assert.equal(videoSummary({ height: 1080, fps: 60, quality: 50 }, "x264", false), "1080p60 · Smallest · x264");
  assert.equal(audioSummary(1), "1 audio source");
  assert.equal(audioSummary(3), "3 audio sources");
});
