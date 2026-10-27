import { test } from "node:test";
import assert from "node:assert/strict";
import { Outbox, enqueue, onTop, type Write } from "./outbox.ts";

interface Cfg {
  revision: number;
  x: boolean;
  y: number;
  rows: Record<string, number>;
}

const cfg = (revision: number, change: Partial<Cfg> = {}): Cfg => ({
  revision,
  x: false,
  y: 0,
  rows: {},
  ...change,
});

/** A promise to settle by hand, for an answer still on its way. */
function later<T>() {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const tick = () => new Promise((res) => setTimeout(res, 0));

/** A window: what it shows, and every write that left it with its base. */
function windowWith(start: Cfg) {
  let shown: Cfg | null = start;
  const sent: { next: Cfg; base: number; answer: ReturnType<typeof later<Cfg | null>> }[] = [];
  const reads: ReturnType<typeof later<Cfg>>[] = [];
  const box = new Outbox<Cfg>({
    shown: () => shown,
    show: (config) => {
      shown = config;
    },
    read: () => {
      const read = later<Cfg>();
      reads.push(read);
      return read.promise;
    },
  });
  const patch = (change: Partial<Cfg>, failed?: () => boolean): Write<Cfg> => ({
    key: "settings",
    change: (config) => ({ ...config, ...change }),
    send: (next, base) => {
      const answer = later<Cfg | null>();
      sent.push({ next, base: base.revision, answer });
      return answer.promise;
    },
    failed,
  });
  return { box, sent, reads, patch, shown: () => shown! };
}

// --- the pure queue ---------------------------------------------------------

test("patches queued one after another merge, the later on top", () => {
  const a: Write<Cfg> = { key: "s", change: (c) => ({ ...c, x: true, y: 1 }), send: async () => null };
  const b: Write<Cfg> = { key: "s", change: (c) => ({ ...c, x: false }), send: async () => null };
  const queue = enqueue(enqueue([], a), b);
  assert.equal(queue.length, 1);
  assert.deepEqual(onTop(cfg(3), queue), cfg(3, { x: false, y: 1 }));
});

test("writes with another key, or none, keep their own place", () => {
  const a: Write<Cfg> = { key: "s", change: (c) => ({ ...c, y: 1 }), send: async () => null };
  const row: Write<Cfg> = { change: (c) => ({ ...c, rows: { ...c.rows, mic: 1 } }), send: async () => null };
  const b: Write<Cfg> = { key: "s", change: (c) => ({ ...c, y: 2 }), send: async () => null };
  const queue = enqueue(enqueue(enqueue([], a), row), b);
  assert.equal(queue.length, 3);
  assert.deepEqual(onTop(cfg(1), queue), cfg(1, { y: 2, rows: { mic: 1 } }));
});

// --- one write in flight ------------------------------------------------------

test("a switch flipped on and back off while the first write is out ends off", async () => {
  const w = windowWith(cfg(4));
  const on = w.box.push(w.patch({ x: true }));
  const off = w.box.push(w.patch({ x: false }));
  // Only the first went out; the display shows the last thing done.
  assert.equal(w.sent.length, 1);
  assert.equal(w.sent[0].base, 4);
  assert.equal(w.sent[0].next.x, true);
  assert.equal(w.shown().x, false);

  w.sent[0].answer.resolve(cfg(5, { x: true }));
  await on;
  // The second goes out on the answer's revision, so its "off" is a change.
  assert.equal(w.sent.length, 2);
  assert.equal(w.sent[1].base, 5);
  assert.equal(w.sent[1].next.x, false);
  assert.equal(w.sent[1].next.revision, 5);
  // The answer to the first did not flip the display back on meanwhile.
  assert.equal(w.shown().x, false);
  assert.equal(w.shown().revision, 5);

  w.sent[1].answer.resolve(cfg(6, { x: false }));
  await off;
  assert.deepEqual(w.shown(), cfg(6, { x: false }));
  assert.equal(w.box.busy, false);
});

test("a slider dragged while a write is out sends one write per answer", async () => {
  const w = windowWith(cfg(1));
  void w.box.push(w.patch({ y: 10 }));
  for (const y of [20, 30, 40]) void w.box.push(w.patch({ y }));
  assert.equal(w.sent.length, 1);
  assert.equal(w.shown().y, 40);
  w.sent[0].answer.resolve(cfg(2, { y: 10 }));
  await tick();
  assert.equal(w.sent.length, 2);
  assert.equal(w.sent[1].next.y, 40);
  assert.equal(w.sent[1].base, 2);
  w.sent[1].answer.resolve(cfg(3, { y: 40 }));
  await tick();
  assert.equal(w.sent.length, 2);
  assert.equal(w.shown().y, 40);
});

test("the core's answer is the base of the next write, corrections and all", async () => {
  const w = windowWith(cfg(1));
  void w.box.push(w.patch({ x: true }));
  void w.box.push(w.patch({ y: 7 }));
  // The core corrected y on the first write, and a change made elsewhere
  // (row "mic") was merged in.
  w.sent[0].answer.resolve(cfg(2, { x: true, y: 3, rows: { mic: 1 } }));
  await tick();
  assert.deepEqual(w.sent[1].next, cfg(2, { x: true, y: 7, rows: { mic: 1 } }));
});

// --- settings handed over while a write is out ------------------------------

test("settings from elsewhere are held while a write is out, taken after if later", async () => {
  const w = windowWith(cfg(1));
  void w.box.push(w.patch({ x: true }));
  w.box.offer(cfg(2, { rows: { mic: 1 } }));
  // Not shown mid-write: it predates the write and would flip x back.
  assert.equal(w.shown().x, true);
  w.sent[0].answer.resolve(cfg(3, { x: true, rows: { mic: 1 } }));
  await tick();
  // The answer is later than what was held: it already has that change.
  assert.deepEqual(w.shown(), cfg(3, { x: true, rows: { mic: 1 } }));

  void w.box.push(w.patch({ y: 1 }));
  w.box.offer(cfg(5, { x: true, y: 0, rows: { mic: 2 } }));
  w.sent[1].answer.resolve(cfg(4, { x: true, y: 1, rows: { mic: 1 } }));
  await tick();
  // Held one made after the write was taken in: it has the write too.
  assert.equal(w.shown().revision, 5);
  assert.equal(w.shown().rows.mic, 2);
});

test("with nothing on its way, only a later revision replaces what is shown", () => {
  const w = windowWith(cfg(5));
  w.box.offer(cfg(4, { x: true }));
  assert.equal(w.shown().x, false);
  w.box.offer(cfg(6, { x: true }));
  assert.equal(w.shown().x, true);
  // A fresh read stands in for its own revision.
  w.box.read(cfg(6, { y: 9 }));
  assert.equal(w.shown().y, 9);
});

test("after a failed write a held copy is not taken; the settings are read again", async () => {
  const w = windowWith(cfg(1));
  let failures = 0;
  void w.box.push(w.patch({ x: true }, () => (failures++, false)));
  w.box.offer(cfg(2, { y: 5 }));
  w.sent[0].answer.reject(new Error("bridge gone"));
  await tick();
  assert.equal(failures, 1);
  // The held copy may predate the failed write's effect, if any: read instead.
  assert.equal(w.reads.length, 1);
  assert.equal(w.box.busy, true);
  // A write made while the read is out waits behind it.
  void w.box.push(w.patch({ y: 8 }));
  assert.equal(w.sent.length, 1);
  w.reads[0].resolve(cfg(3, { y: 5 }));
  await tick();
  assert.equal(w.sent.length, 2);
  assert.equal(w.sent[1].base, 3);
  assert.deepEqual(w.sent[1].next, cfg(3, { y: 8 }));
});

test("a failed write that asks for it reads the settings again at once", async () => {
  const w = windowWith(cfg(1));
  void w.box.push(w.patch({ x: true }, () => true));
  w.sent[0].answer.reject(new Error("refused"));
  await tick();
  assert.equal(w.reads.length, 1);
  w.reads[0].resolve(cfg(1));
  await tick();
  // The read stands in for the optimistic copy on its own revision.
  assert.deepEqual(w.shown(), cfg(1));
  assert.equal(w.box.busy, false);
});

test("a write resolves once its answer is in, also when it merged with the next", async () => {
  const w = windowWith(cfg(1));
  let first = false;
  let second = false;
  let third = false;
  void w.box.push(w.patch({ y: 1 })).then(() => (first = true));
  void w.box.push(w.patch({ y: 2 })).then(() => (second = true));
  void w.box.push(w.patch({ y: 3 })).then(() => (third = true));
  w.sent[0].answer.resolve(cfg(2, { y: 1 }));
  await tick();
  assert.deepEqual([first, second, third], [true, false, false]);
  w.sent[1].answer.resolve(cfg(3, { y: 3 }));
  await tick();
  assert.deepEqual([first, second, third], [true, true, true]);
});

test("a write with nothing left to send settles without an answer", async () => {
  const w = windowWith(cfg(1));
  await w.box.push({ change: (c) => c, send: async () => null });
  assert.equal(w.box.busy, false);
  assert.deepEqual(w.shown(), cfg(1));
});
