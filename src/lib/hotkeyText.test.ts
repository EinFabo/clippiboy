import { test } from "node:test";
import assert from "node:assert/strict";
import { hotkeyKeys, hotkeyLabel, isHold, isPad, keyLabel, triggerOf, withHold } from "./hotkeyText.ts";

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
  assert.equal(isPad("Hold:Pad:LB+RB+A"), true);
  assert.deepEqual(hotkeyKeys("Pad:LB+RB+A"), ["LB", "RB", "A"]);
  assert.equal(hotkeyLabel("Hold:Pad:View+A"), "Hold View + A");
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
