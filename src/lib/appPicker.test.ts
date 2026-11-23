import { test } from "node:test";
import assert from "node:assert/strict";
import { appRows, deviceAdded } from "./appPicker.ts";
import type { AudioProcess, AudioSource } from "./types.ts";

const proc = (pid: number, exe: string, count = 1): AudioProcess => ({
  pid,
  name: exe.replace(".exe", ""),
  exe,
  count,
  active: true,
});
const added = (exe: string | null): AudioSource => ({
  id: "a",
  label: "x",
  kind: { type: "process", pid: 1, exe, mode: "include" },
  enabled: true,
  gainDb: 0,
  muted: false,
  solo: false,
  separateTrack: false,
});

test("an application that is a source already is marked, whatever the case", () => {
  const rows = appRows([proc(1, "Discord.exe"), proc(2, "cs2.exe")], [added("discord.EXE")], null);
  assert.deepEqual(rows.map((r) => r.added), [true, false]);
});

test("the process count shows only above one, and the game is marked", () => {
  const rows = appRows([proc(1, "Discord.exe", 3), proc(2, "cs2.exe")], [], "cs2");
  assert.equal(rows[0].sub, "Discord.exe · 3 processes");
  assert.equal(rows[1].sub, "cs2.exe · Detected game");
  assert.equal(rows[1].isGame, true);
});

test("an older entry without a count reads as one process", () => {
  const rows = appRows([{ pid: 1, name: "A", exe: "a.exe" }], [], null);
  assert.equal(rows[0].sub, "a.exe");
});

test("a device that is a source already is marked, by id or as the default it follows", () => {
  const source = (type: "outputDevice" | "inputDevice", deviceId: string): AudioSource => ({
    ...added(null),
    kind: { type, deviceId },
  });
  const speakers = { id: "out-1", isDefault: true };
  const headset = { id: "out-2", isDefault: false };
  assert.equal(deviceAdded([source("outputDevice", "out-2")], "outputDevice", headset), true);
  assert.equal(deviceAdded([source("outputDevice", "out-2")], "outputDevice", speakers), false);
  // A source following the Windows default takes the default's row.
  assert.equal(deviceAdded([source("outputDevice", "")], "outputDevice", speakers), true);
  assert.equal(deviceAdded([source("outputDevice", "")], "outputDevice", headset), false);
  // An input with the same id is another kind of source.
  assert.equal(deviceAdded([source("inputDevice", "out-1")], "outputDevice", speakers), false);
  assert.equal(deviceAdded([], "inputDevice", speakers), false);
});
