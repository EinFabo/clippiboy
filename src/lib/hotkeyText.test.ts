import { test } from "node:test";
import assert from "node:assert/strict";
import {
  hotkeyKeys,
  hotkeyLabel,
  isHold,
  isPad,
  keyLabel,
  padButton,
  padLabel,
  placeBinding,
  switchMode,
  triggerKey,
  triggerOf,
  withHold,
} from "./hotkeyText.ts";

test("an old keyboard binding reads as before", () => {
  assert.deepEqual(hotkeyKeys("Ctrl+Shift+S"), ["Ctrl", "Shift", "S"]);
  assert.equal(hotkeyLabel("Alt+PrintScreen"), "Alt + PrtSc");
  assert.equal(hotkeyLabel("Numpad1"), "Num 1");
  assert.equal(isHold("Ctrl+Shift+S"), false);
  assert.equal(hotkeyLabel(""), "");
});

test("mouse buttons get their printed names", () => {
  assert.equal(keyLabel("Mouse4"), "Mouse 4");
  assert.equal(hotkeyLabel("Ctrl+Mouse5"), "Ctrl + Mouse 5");
  assert.equal(isPad("Mouse3"), false);
});

test("controller combos drop the prefix", () => {
  assert.equal(isPad("Pad:View+A"), true);
  assert.equal(isPad("Hold:Pad:LB+RB+South"), true);
  assert.deepEqual(hotkeyKeys("Pad:LB+RB+South"), ["LB", "RB", "A"]);
  assert.equal(hotkeyLabel("Hold:Pad:Back+South"), "Hold View + A");
});

test("old bindings in Xbox names read as before", () => {
  assert.deepEqual(hotkeyKeys("Pad:LB+RB+A"), ["LB", "RB", "A"]);
  assert.equal(hotkeyLabel("Hold:Pad:View+A"), "Hold View + A");
  assert.equal(padButton("View"), "Back");
  assert.equal(padButton("a"), "South");
  assert.equal(padButton("Xbox"), "Guide");
});

test("a single button and Guide read on their own", () => {
  assert.deepEqual(hotkeyKeys("Pad:Guide"), ["Xbox"]);
  assert.equal(hotkeyLabel("Hold:Pad:Misc"), "Hold Share");
  assert.equal(hotkeyLabel("Pad:Up"), "Up");
});

test("buttons carry the names printed on the pad in use", () => {
  assert.equal(hotkeyLabel("Pad:Back+South", "playstation"), "Share + Cross");
  assert.equal(hotkeyLabel("Pad:Guide", "playstation"), "PS");
  assert.equal(hotkeyLabel("Pad:LB+RT", "playstation"), "L1 + R2");
  // By position: the bottom button is B on a Nintendo pad, the right one A.
  assert.equal(padLabel("South", "nintendo"), "B");
  assert.equal(padLabel("East", "nintendo"), "A");
  assert.equal(padLabel("A", "nintendo"), "B");
  assert.equal(hotkeyLabel("Pad:Misc", "nintendo"), "Capture");
  assert.equal(padLabel("Guide", "nintendo"), "Home");
  assert.equal(hotkeyLabel("Pad:East+West", "gamecube"), "X + B");
  // What a family has no name of its own for, Xbox's name stands in.
  assert.equal(padLabel("Touchpad", "nintendo"), "Touchpad");
  assert.equal(padLabel("LS", "nintendo"), "LS");
});

test("tap and hold switch without touching what is pressed", () => {
  assert.equal(isHold("Hold:Mouse5"), true);
  assert.equal(isHold("hold: Mouse5"), true);
  assert.equal(triggerOf("Hold:Mouse5"), "Mouse5");
  assert.equal(withHold("Mouse5", true), "Hold:Mouse5");
  assert.equal(withHold("Hold:Mouse5", false), "Mouse5");
  assert.equal(withHold("Hold:Ctrl+S", true), "Hold:Ctrl+S");
  assert.equal(withHold("", true), "");
});

test("one input in two spellings is the same input", () => {
  assert.equal(triggerKey("ctrl+s"), triggerKey("Hold:Ctrl+S"));
  assert.equal(triggerKey("Control+KeyS"), triggerKey("Ctrl+S"));
  assert.equal(triggerKey("Pad:View+A"), triggerKey("Hold:Pad:Back+South"));
  assert.equal(triggerKey("Shift+Ctrl+Mouse5"), triggerKey("Ctrl+Shift+Mouse5"));
  assert.notEqual(triggerKey("Ctrl+Mouse5"), triggerKey("Mouse5"));
  assert.notEqual(triggerKey("Pad:South"), triggerKey("Pad:Back+South"));
  assert.equal(triggerKey(""), "");
});

const bound = (changes: Record<string, string>) => ({
  saveClip: "",
  screenshot: "",
  record: "",
  screenshotAlt: "",
  ...changes,
});

test("a second action on the same input takes the free way", () => {
  // Mouse5 tapped saves a clip: recorded for the screenshot, it becomes held.
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse5" }), "screenshot", "Mouse5"), {
    screenshot: "Hold:Mouse5",
  });
  // And the other way round, in another spelling, in the second slot.
  assert.deepEqual(
    placeBinding(bound({ saveClip: "Hold:Pad:View+A" }), "screenshotAlt", "Pad:Back+South"),
    { screenshotAlt: "Pad:Back+South" },
  );
  assert.deepEqual(
    placeBinding(bound({ saveClip: "Pad:Back+South" }), "screenshotAlt", "Pad:View+A"),
    { screenshotAlt: "Hold:Pad:View+A" },
  );
  // A free input, or the same binding recorded again on its own field: as it is.
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse4" }), "screenshot", "Mouse5"), {
    screenshot: "Mouse5",
  });
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse5" }), "saveClip", "Mouse5"), {
    saveClip: "Mouse5",
  });
});

test("a third action on an input with both ways taken stays a clash", () => {
  const both = bound({ saveClip: "Mouse5", screenshot: "Hold:Mouse5" });
  assert.deepEqual(placeBinding(both, "record", "Mouse5"), { record: "Mouse5" });
  assert.deepEqual(placeBinding(both, "record", "Hold:Mouse5"), { record: "Hold:Mouse5" });
});

test("switching tap and hold swaps with the action sharing the input", () => {
  const pair = bound({ saveClip: "Mouse5", screenshot: "Hold:Mouse5" });
  assert.deepEqual(switchMode(pair, "saveClip", true), {
    saveClip: "Hold:Mouse5",
    screenshot: "Mouse5",
  });
  assert.deepEqual(switchMode(pair, "screenshot", false), {
    screenshot: "Mouse5",
    saveClip: "Hold:Mouse5",
  });
  // Alone on its input: only itself; already that way: nothing.
  assert.deepEqual(switchMode(bound({ record: "F9" }), "record", true), { record: "Hold:F9" });
  assert.deepEqual(switchMode(pair, "saveClip", false), {});
});
