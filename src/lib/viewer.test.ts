import { test } from "node:test";
import assert from "node:assert/strict";
import { leaving, menuOwns, sliderOwns, typesText } from "./viewer.ts";

test("Escape and the arrows belong to an open menu", () =>
  assert.deepEqual(["Escape", "ArrowUp", "ArrowDown"].map((k) => menuOwns(k, -1)), [true, true, true]));
test("Enter belongs to the menu only with an entry picked", () => {
  assert.equal(menuOwns("Enter", -1), false);
  assert.equal(menuOwns("Enter", 0), true);
});
test("other keys pass on", () => assert.equal(menuOwns("k", 2), false));
test("a slider is not a text field", () => {
  assert.equal(typesText({ tagName: "INPUT", type: "range" }), false);
  assert.equal(typesText({ tagName: "INPUT", type: "text" }), true);
  assert.equal(typesText({ tagName: "TEXTAREA" }), true);
  assert.equal(typesText(null), false);
});
test("a focused slider keeps the keys that move it", () => {
  const slider = { tagName: "INPUT", type: "range" };
  for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
    assert.equal(sliderOwns(slider, key), true, key);
  }
  assert.equal(sliderOwns(slider, " "), false);
  assert.equal(sliderOwns(slider, "i"), false);
  assert.equal(sliderOwns({ tagName: "VIDEO" }, "ArrowLeft"), false);
  assert.equal(sliderOwns(null, "ArrowLeft"), false);
});
test("unapplied edits are asked about before the player closes", () => assert.equal(leaving(false, true), "ask"));
test("a running write keeps the player open", () => assert.equal(leaving(true, false), "wait"));
test("with nothing changed it just closes", () => assert.equal(leaving(false, false), "go"));
