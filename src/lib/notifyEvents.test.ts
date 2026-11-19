import { test } from "node:test";
import assert from "node:assert/strict";
import { NOTIFY_EVENTS } from "./notifyEvents.ts";
import type { SoundKind } from "./types.ts";

// Every key of SoundConfig that is a sound; the compiler checks this list is complete.
const ALL: Record<SoundKind, true> = {
  clipSaved: true,
  screenshot: true,
  recordingStarted: true,
  recordingSaved: true,
  bufferOn: true,
  bufferOff: true,
  friendOnline: true,
  clipReceived: true,
  error: true,
};

test("every sound kind has exactly one row", () => {
  const used = NOTIFY_EVENTS.flatMap((e) => (e.sound ? [e.sound] : [])).sort();
  assert.deepEqual(used, Object.keys(ALL).sort());
});

test("labels are unique and every row has a banner or a sound", () => {
  const labels = NOTIFY_EVENTS.map((e) => e.label);
  assert.equal(new Set(labels).size, labels.length);
  for (const e of NOTIFY_EVENTS) assert.ok(e.banner || e.sound, e.label);
});

test("shared banner fields appear on exactly the rows that share them", () => {
  const shared = (key: string) =>
    NOTIFY_EVENTS.filter((e) => e.banner?.key === key).map((e) => e.label);
  assert.deepEqual(shared("onRecording"), ["Recording started", "Recording saved"]);
  assert.deepEqual(shared("onBufferToggle"), ["Buffer on", "Buffer off"]);
});
