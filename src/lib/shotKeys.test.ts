import { test } from "node:test";
import assert from "node:assert/strict";
import { shortcut } from "./shotKeys.ts";

const key = (k: string, mods: { ctrl?: boolean; shift?: boolean; alt?: boolean } = {}) => ({
  key: k,
  ctrlKey: mods.ctrl ?? false,
  metaKey: false,
  shiftKey: mods.shift ?? false,
  altKey: mods.alt ?? false,
});

test("a letter picks its tool", () => {
  assert.deepEqual(shortcut(key("v")), { tool: "select" });
  assert.deepEqual(shortcut(key("a")), { tool: "arrow" });
  assert.deepEqual(shortcut(key("r")), { tool: "box" });
  assert.deepEqual(shortcut(key("e")), { tool: "ellipse" });
  assert.deepEqual(shortcut(key("p")), { tool: "pen" });
  assert.deepEqual(shortcut(key("t")), { tool: "text" });
  assert.deepEqual(shortcut(key("b")), { tool: "blur" });
  assert.deepEqual(shortcut(key("B")), { tool: "blur" }, "Caps Lock on");
});

test("with a modifier a letter is no tool", () => {
  assert.equal(shortcut(key("a", { alt: true })), null);
  assert.equal(shortcut(key("x")), null);
  assert.equal(shortcut(key("p", { ctrl: true })), null);
});

test("undo, redo, save and copy", () => {
  assert.equal(shortcut(key("z", { ctrl: true })), "undo");
  assert.equal(shortcut(key("Z", { ctrl: true })), "undo", "Caps Lock on");
  assert.equal(shortcut(key("z", { ctrl: true, shift: true })), "redo");
  assert.equal(shortcut(key("y", { ctrl: true })), "redo");
  assert.equal(shortcut(key("s", { ctrl: true })), "save");
  assert.equal(shortcut(key("c", { ctrl: true })), "copy");
  assert.equal(shortcut(key("c", { ctrl: true, shift: true })), null);
});
