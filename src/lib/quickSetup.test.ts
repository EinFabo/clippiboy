import { test } from "node:test";
import assert from "node:assert/strict";
import { QUICK_PICKS, quickAdds, quickOffer, type QuickFacts } from "./quickSetup.ts";
import type { AudioDevice, AudioSource, SourceKind } from "./types.ts";

const dev = (id: string, kind: "output" | "input", isDefault = true): AudioDevice => ({
  id,
  name: `Name ${id}`,
  kind,
  isDefault,
});
const src = (kind: SourceKind): AudioSource => ({
  id: "s",
  label: "s",
  kind,
  enabled: true,
  gainDb: 0,
  muted: false,
  solo: false,
  separateTrack: true,
});
const discord = { pid: 7, name: "Discord", exe: "Discord.exe" };
const facts = (over: Partial<QuickFacts> = {}): QuickFacts => ({
  devices: [dev("spk", "output"), dev("other", "output", false), dev("mic", "input")],
  sources: [],
  processes: [],
  appCapture: true,
  ...over,
});

test("a fresh install gets the default output and microphone, named after them", () => {
  assert.deepEqual(quickAdds(facts(), QUICK_PICKS), [
    { label: "Name spk", kind: { type: "outputDevice", deviceId: "spk" } },
    { label: "Name mic", kind: { type: "inputDevice", deviceId: "mic" } },
  ]);
});

test("a switch turned off is left out", () => {
  const adds = quickAdds(facts(), { ...QUICK_PICKS, input: false });
  assert.deepEqual(adds.map((a) => a.kind.type), ["outputDevice"]);
  assert.deepEqual(quickAdds(facts(), { output: false, input: false, discord: false }), []);
});

test("what the list holds already is not added a second time", () => {
  const sources = [src({ type: "outputDevice", deviceId: "spk" })];
  const offer = quickOffer(facts({ sources }));
  assert.equal(offer.output?.added, true);
  assert.equal(offer.input?.added, false);
  assert.deepEqual(quickAdds(facts({ sources }), QUICK_PICKS).map((a) => a.kind.type), ["inputDevice"]);
});

test("a running Discord comes along on its own track, only with per-app capture", () => {
  const adds = quickAdds(facts({ processes: [discord] }), QUICK_PICKS);
  assert.deepEqual(adds[2], {
    label: "Discord",
    kind: { type: "process", pid: 7, exe: "Discord.exe", mode: "include" },
  });
  assert.equal(quickAdds(facts({ processes: [discord], appCapture: false }), QUICK_PICKS).length, 2);
  // Not answered yet counts as possible, as the row shows it.
  assert.equal(quickAdds(facts({ processes: [discord], appCapture: null }), QUICK_PICKS).length, 3);
});

test("Discord on a track already shows as added, running or not", () => {
  const sources = [src({ type: "process", pid: 1, exe: "discord.exe", mode: "include" })];
  assert.deepEqual(quickOffer(facts({ sources })).discord, { process: undefined, added: true });
  assert.equal(quickAdds(facts({ sources, processes: [discord] }), QUICK_PICKS).length, 2);
});

test("without default devices there is nothing to offer", () => {
  const none = facts({ devices: [dev("x", "output", false)] });
  assert.deepEqual(quickOffer(none), {});
  assert.deepEqual(quickAdds(none, QUICK_PICKS), []);
});
