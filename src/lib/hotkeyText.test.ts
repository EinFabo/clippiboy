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
