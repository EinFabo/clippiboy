import { test } from "node:test";
import assert from "node:assert/strict";
import { bannerSummary, blockedSummary, camAppearsSummary, camPositionSummary, controllerSummary, soundsSummary, streamDeckSummary } from "./settingsSummary.ts";
import type { PadBindings, PadProfiles } from "./types.ts";

const none: PadBindings = { saveClip: "", toggleBuffer: "", screenshot: "", record: "", console: "" };
const profiles = (over: Partial<PadProfiles> = {}): PadProfiles => ({
  all: { ...none, saveClip: "Pad:South", console: "Pad:Guide" },
  xbox: null,
  playstation: null,
  nintendo: null,
  ...over,
});

test("banner summary", () => {
  assert.equal(bannerSummary("Main screen", "bottomRight", 3500), "Main screen · bottom right · 3.5 s");
  assert.equal(bannerSummary("Follows the game", "topLeft", 2000), "Follows the game · top left · 2 s");
});

test("sounds summary", () => {
  assert.equal(soundsSummary(null, 0.8), "Windows default · 80 %");
  assert.equal(soundsSummary("Headset", 0.456), "Headset · 46 %");
});

test("stream deck summary", () => {
  assert.equal(streamDeckSummary(true, 47653), "Allowed · port 47653");
  assert.equal(streamDeckSummary(false, 47653), "Off");
});

test("controller summary", () => {
  assert.equal(controllerSummary(profiles(), [], [], null), "No controller seen");
  assert.equal(controllerSummary(profiles(), ["xbox"], [], "xbox"), "Xbox · 2 buttons bound");
  assert.equal(
    controllerSummary(profiles({ playstation: { ...none, record: "Pad:South" } }), ["playstation"], ["playstation"], null),
    "PlayStation · 1 button bound",
  );
  assert.equal(controllerSummary(profiles(), ["other"], [], "other"), "Other · 2 buttons bound");
});

test("camera summaries", () => {
  assert.equal(camAppearsSummary(true, true, false, false), "Clips · Recordings");
  assert.equal(camAppearsSummary(true, true, true, true), "Clips · Recordings · Screenshots · Console");
  assert.equal(camAppearsSummary(false, false, false, false), "Nowhere");
  assert.equal(camPositionSummary("circle", true, 0), "Circle · mirrored");
  assert.equal(camPositionSummary("rect", false, 0), "Rectangle");
  assert.equal(camPositionSummary("rounded", false, -40), "Rounded · \u221240 ms");
  assert.equal(camPositionSummary("rounded", true, 120), "Rounded · mirrored · +120 ms");
});

test("blocked summary", () => {
  assert.equal(blockedSummary(0), "Nobody");
  assert.equal(blockedSummary(3), "3 blocked");
});
