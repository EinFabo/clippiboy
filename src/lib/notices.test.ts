import { test } from "node:test";
import assert from "node:assert/strict";
import { current, neighbour, queue } from "./notices.ts";

test("nothing wants to show", () => {
  assert.deepEqual(queue({}), []);
  assert.equal(current({}, null), null);
  assert.equal(neighbour({}, null, 1), null);
});
test("the order is ffmpeg, update, usage, Discord whatever the order of asking", () => {
  assert.deepEqual(queue({ discord: true, usage: true, update: true, ffmpeg: true }), [
    "ffmpeg",
    "update",
    "usage",
    "discord",
  ]);
});
test("the most important one shows first", () => {
  assert.deepEqual(current({ usage: true, discord: true }, null), { id: "usage", index: 0, count: 2 });
});
test("a pick shows while it still wants to", () => {
  assert.deepEqual(current({ update: true, discord: true }, "discord"), {
    id: "discord",
    index: 1,
    count: 2,
  });
});
test("a pick that went away falls back to the first", () => {
  assert.deepEqual(current({ update: true }, "discord"), { id: "update", index: 0, count: 1 });
});
test("false wants do not count", () => {
  assert.deepEqual(queue({ ffmpeg: false, usage: true }), ["usage"]);
});
test("the pager wraps at both ends", () => {
  const w = { ffmpeg: true, update: true, usage: true };
  assert.equal(neighbour(w, null, -1), "usage");
  assert.equal(neighbour(w, "usage", 1), "ffmpeg");
  assert.equal(neighbour(w, "update", 1), "usage");
});
