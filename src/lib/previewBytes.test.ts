import { test } from "node:test";
import assert from "node:assert/strict";
import { previewBytes } from "./previewBytes.ts";

test("an ArrayBuffer is passed on as it is", () => {
  const buffer = new Uint8Array([1, 2, 3]).buffer;
  assert.equal(previewBytes(buffer), buffer);
});

// Tauri's fallback channel (the one the release used while its CSP blocked
// http://ipc.localhost) hands raw bytes over as a list of numbers.
test("a list of numbers becomes the same bytes", () => {
  const bytes = new Uint8Array(previewBytes([7, 0, 255]));
  assert.deepEqual([...bytes], [7, 0, 255]);
});
