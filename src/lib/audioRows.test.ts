import { test } from "node:test";
import assert from "node:assert/strict";
import { appCaptureUnavailable, gainText, sourceStatus, stuckSolo } from "./audioRows.ts";
import type { StatusContext } from "./audioRows.ts";
import type { AudioSource, SourceKind } from "./types.ts";

const src = (id: string, kind: SourceKind, solo = false): AudioSource => ({
  id,
  label: id,
  kind,
  enabled: true,
  gainDb: 0,
  muted: false,
  solo,
  separateTrack: false,
});
const ctx = (over: Partial<StatusContext> = {}): StatusContext => ({
  devices: [{ id: "in-1", name: "Elgato Wave XLR", kind: "input", isDefault: true }],
  processes: [{ pid: 10, name: "Discord", exe: "Discord.exe" }],
  detectedGame: null,
  tapped: [],
  leftovers: false,
  ...over,
});
const proc = (pid: number, exe: string | null, mode: "include" | "exclude" = "include"): SourceKind => ({
  type: "process",
  pid,
  exe,
  mode,
});

test("stuckSolo finds only sources with Solo on", () => {
  const a = src("a", { type: "game" });
  const b = src("b", { type: "game" }, true);
  assert.deepEqual(stuckSolo([a, b]), [b]);
  assert.deepEqual(stuckSolo([a]), []);
});

test("an application that is running is named, one that is gone waits", () => {
  assert.deepEqual(sourceStatus(src("d", proc(10, "Discord.exe")), ctx()), {
    text: "Discord · running",
    tone: "plain",
  });
  // The pid changed with a restart; the exe still finds it.
  assert.equal(sourceStatus(src("d", proc(99, "discord.exe")), ctx()).text, "Discord · running");
  assert.deepEqual(sourceStatus(src("s", proc(5, "Spotify.exe")), ctx()), {
    text: "Not running · waiting",
    tone: "warn",
  });
});

test("everything-except names what is left out", () => {
  assert.equal(
    sourceStatus(src("x", proc(10, "Discord.exe", "exclude")), ctx()).text,
    "Everything except Discord",
  );
});

test("the game source follows the detected game", () => {
  assert.equal(
    sourceStatus(src("g", { type: "game" }), ctx({ detectedGame: "Counter-Strike 2" })).text,
    "Counter-Strike 2 · running",
  );
  assert.equal(sourceStatus(src("g", { type: "game" }), ctx()).tone, "warn");
});

test("a device shows its name, a missing one is unplugged", () => {
  assert.equal(
    sourceStatus(src("m", { type: "inputDevice", deviceId: "in-1" }), ctx()).text,
    "Elgato Wave XLR",
  );
  assert.deepEqual(sourceStatus(src("m", { type: "inputDevice", deviceId: "gone" }), ctx()), {
    text: "Unplugged",
    tone: "warn",
  });
  assert.equal(
    sourceStatus(src("m", { type: "inputDevice", deviceId: "" }), ctx()).text,
    "Windows default",
  );
  // Before the first device list nothing can be called unplugged.
  assert.equal(
    sourceStatus(src("m", { type: "inputDevice", deviceId: "gone" }), ctx({ devices: [] })).tone,
    "plain",
  );
});

test("an output in leftovers mode says what it holds", () => {
  const out = src("o", { type: "outputDevice", deviceId: "o1" });
  const devices = [{ id: "o1", name: "Headphones", kind: "output" as const, isDefault: true }];
  assert.equal(sourceStatus(out, ctx({ devices, leftovers: true, tapped: ["Chrome"] })).text, "Headphones · Chrome");
  assert.equal(sourceStatus(out, ctx({ devices, leftovers: true })).text, "Headphones · nothing left");
  assert.equal(sourceStatus(out, ctx({ devices })).text, "Headphones");
});

test("the core's error and warning win over the plain line", () => {
  const d = src("d", proc(10, "Discord.exe"));
  assert.deepEqual(sourceStatus(d, ctx({ error: "boom", warning: "hm" })), { text: "boom", tone: "error" });
  assert.deepEqual(sourceStatus(d, ctx({ warning: "hm" })), { text: "hm", tone: "warn" });
});

test("application capture is unavailable only on an application source's error", () => {
  const app = src("a", proc(1, "a.exe"));
  const dev = src("m", { type: "inputDevice", deviceId: "x" });
  const named = "process loopback needs Windows build 20348 or newer";
  assert.equal(appCaptureUnavailable([app], { a: named }), true);
  assert.equal(appCaptureUnavailable([dev], { m: named }), false);
  // What the core says for any source that cannot start says nothing about the Windows build.
  assert.equal(appCaptureUnavailable([app], { a: "audio source not available: The parameter is incorrect" }), false);
  assert.equal(appCaptureUnavailable([app], { a: "loopback device not available" }), false);
  assert.equal(appCaptureUnavailable([app], {}), false);
});

test("gain text carries a sign only above zero", () => {
  assert.equal(gainText(2), "+2.0 dB");
  assert.equal(gainText(0), "0.0 dB");
  assert.equal(gainText(-3.5), "-3.5 dB");
});
