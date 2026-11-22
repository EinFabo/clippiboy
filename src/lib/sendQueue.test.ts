import { test } from "node:test";
import assert from "node:assert/strict";
import {
  appeared,
  awaitsAnswer,
  dropQueue,
  enqueue,
  retrying,
  step,
  stopQueue,
  waitingFor,
  type Queued,
  type SendQueue,
} from "./sendQueue.ts";
import type { Transfer } from "./types.ts";

const q = (clipId: string, over: Partial<Queued> = {}): Queued => ({
  clipId,
  name: clipId,
  screenshot: false,
  recording: false,
  ...over,
});

const t = (over: Partial<Transfer>): Transfer => ({
  id: "t",
  direction: "out",
  friendId: "1",
  friendName: "Luca",
  name: "Ace on Mirage",
  size: 100,
  moved: 0,
  stage: "asking",
  error: null,
  clipId: "a",
  game: null,
  durationMs: 0,
  screenshot: false,
  recording: false,
  ...over,
});

test("queue: a clip is queued once", () => {
  const one = enqueue({}, "1", [q("a"), q("b")]);
  assert.deepEqual(waitingFor(one, "1").map((x) => x.clipId), ["a", "b"]);
  const again = enqueue(one, "1", [q("b"), q("c"), q("c")]);
  assert.deepEqual(waitingFor(again, "1").map((x) => x.clipId), ["a", "b", "c"]);
  assert.equal(enqueue(again, "1", [q("a")]), again);
  assert.deepEqual(waitingFor(dropQueue(again, "1"), "1"), []);
  assert.equal(dropQueue(again, "2"), again);
});

test("queue: one clip at a time, the next once the friend has said yes", () => {
  let queue: SendQueue = enqueue({}, "1", [q("a"), q("b")]);
  // Nothing on its way yet: the first goes.
  let s = step(queue, []);
  assert.deepEqual(s.send, [{ friendId: "1", item: q("a") }]);
  queue = s.queue;
  // Handed over, not in the list yet: still shown as waiting, nothing more goes.
  assert.deepEqual(waitingFor(queue, "1").map((x) => x.clipId), ["a", "b"]);
  s = step(queue, []);
  assert.deepEqual(s.send, []);
  assert.equal(s.queue, queue);
  // Turned up, waiting for the answer.
  s = step(queue, [t({ id: "ta", stage: "preparing" })]);
  assert.deepEqual(s.send, []);
  queue = s.queue;
  assert.deepEqual(waitingFor(queue, "1").map((x) => x.clipId), ["b"]);
  assert.equal(step(queue, [t({ id: "ta" })]).queue, queue);
  // Accepted: the next one goes.
  s = step(queue, [t({ id: "ta", stage: "moving" })]);
  assert.deepEqual(s.send, [{ friendId: "1", item: q("b") }]);
  queue = s.queue;
  s = step(queue, [t({ id: "ta", stage: "done" }), t({ id: "tb", clipId: "b", stage: "moving" })]);
  // All handed over and answered: the chain is gone.
  assert.deepEqual(s.queue, {});
  assert.deepEqual(s.stopped, []);
});

test("queue: an earlier transfer of the same clip is not taken for the new one", () => {
  const old = t({ id: "old", stage: "failed" });
  const s = step(enqueue({}, "1", [q("a"), q("b")]), [old]);
  assert.deepEqual(s.send.map((x) => x.item.clipId), ["a"]);
  // Only the old failure in the list: the new one has not turned up yet.
  assert.deepEqual(step(s.queue, [old]).send, []);
  assert.deepEqual(step(s.queue, [old]).stopped, []);
});

test("queue: a no, a failure, no answer or a cancel ends the chain", () => {
  for (const stage of ["declined", "failed", "expired", "cancelled"] as const) {
    const first = step(enqueue({}, "1", [q("a"), q("b"), q("c")]), []);
    const s = step(first.queue, [t({ id: "ta", stage })]);
    assert.deepEqual(s.send, [], stage);
    assert.deepEqual(s.stopped, [{ friendId: "1", left: [q("b"), q("c")] }], stage);
    assert.deepEqual(s.queue, {}, stage);
  }
  // Gone from the list before it was answered — the same.
  const first = step(enqueue({}, "1", [q("a"), q("b")]), []);
  const seen = step(first.queue, [t({ id: "ta" })]).queue;
  assert.deepEqual(step(seen, []).stopped, [{ friendId: "1", left: [q("b")] }]);
});

test("queue: waits for an offer sent some other way; friends go side by side", () => {
  const queue = enqueue(enqueue({}, "1", [q("a")]), "2", [q("x")]);
  const s = step(queue, [t({ id: "other", clipId: "z", stage: "asking" })]);
  assert.deepEqual(s.send, [{ friendId: "2", item: q("x") }]);
  assert.deepEqual(waitingFor(s.queue, "1").map((x) => x.clipId), ["a"]);
});

test("queue: an offer waiting for an answer holds the next one back", () => {
  assert.equal(awaitsAnswer([t({ stage: "preparing" })], "1"), true);
  assert.equal(awaitsAnswer([t({ stage: "moving" })], "1"), false);
  assert.equal(awaitsAnswer([t({ direction: "in" })], "1"), false);
  assert.equal(awaitsAnswer([t({})], "2"), false);
});

test("queue: nothing goes twice — handed over, on its way, or a second Retry", () => {
  // The clip just handed to the core.
  const handed = step(enqueue({}, "1", [q("a")]), []).queue;
  assert.equal(enqueue(handed, "1", [q("a")]), handed);
  // Still on its way to them, sent some other way.
  for (const stage of ["preparing", "asking", "moving"] as const)
    assert.deepEqual(enqueue({}, "1", [q("a")], [t({ stage })]), {}, stage);
  // Over: it may go again.
  assert.deepEqual(waitingFor(enqueue({}, "1", [q("a")], [t({ stage: "failed" })]), "1").length, 1);
  // Retry clicked twice: the failure is queued once, and Retry knows it.
  const once = enqueue({}, "1", [q("a", { replaces: "f" })]);
  assert.equal(enqueue(once, "1", [q("a", { replaces: "f" })]), once);
  assert.equal(retrying(once, "f"), true);
  const sent = step(once, []).queue;
  assert.equal(retrying(sent, "f"), true);
  assert.equal(enqueue(sent, "1", [q("b", { replaces: "f" })]), sent);
  assert.equal(retrying(sent, "g"), false);
});

test("queue: the X before the handed-over clip turns up cancels it when it does", () => {
  let queue = step(enqueue({}, "1", [q("a"), q("b")]), []).queue;
  queue = stopQueue(queue, "1");
  assert.deepEqual(waitingFor(queue, "1"), []);
  let s = step(queue, []);
  assert.deepEqual(s.cancel, []);
  s = step(queue, [t({ id: "ta", stage: "preparing" })]);
  assert.deepEqual(s.cancel, ["ta"]);
  assert.deepEqual(s.send, []);
  assert.deepEqual(s.queue, {});
  // Clips picked again after the X wait for the cancelled one to clear.
  queue = enqueue(stopQueue(step(enqueue({}, "1", [q("a")]), []).queue, "1"), "1", [q("c")]);
  s = step(queue, [t({ id: "ta", stage: "preparing" })]);
  assert.deepEqual(s.cancel, ["ta"]);
  assert.deepEqual(s.send, []);
  s = step(s.queue, [t({ id: "ta", stage: "cancelled" })]);
  assert.deepEqual(s.send.map((x) => x.item.clipId), ["c"]);
  // Stopped once its transfer is known: the X cancels that itself, the chain just goes.
  const known = step(step(enqueue({}, "1", [q("a"), q("b")]), []).queue, [t({ id: "ta" })]).queue;
  assert.deepEqual(stopQueue(known, "1"), {});
});

test("queue: a clip gone from the library is skipped, the chain goes on", () => {
  const s = step(enqueue({}, "1", [q("gone"), q("b")]), [], (id) => id !== "gone");
  assert.deepEqual(s.skipped, [{ friendId: "1", item: q("gone") }]);
  assert.deepEqual(s.send.map((x) => x.item.clipId), ["b"]);
  const all = step(enqueue({}, "1", [q("gone")]), [], () => false);
  assert.deepEqual(all.queue, {});
  assert.deepEqual(all.send, []);
});

test("queue: a failed send reports only when its transfer never turned up", () => {
  assert.equal(appeared([t({ id: "old" })], "1", "a", ["old"]), undefined);
  assert.equal(appeared([t({ id: "old" }), t({ id: "new", stage: "failed" })], "1", "a", ["old"])?.id, "new");
  assert.equal(appeared([t({ id: "new" })], "2", "a", []), undefined);
  assert.equal(appeared([t({ id: "new", clipId: "b" })], "1", "a", []), undefined);
});
