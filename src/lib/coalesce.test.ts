import assert from "node:assert/strict";
import test from "node:test";
import { coalesce } from "./coalesce.ts";

test("a burst of changes makes one read and one more after it", async () => {
  let runs = 0;
  let release!: () => void;
  const reread = coalesce(() => {
    runs++;
    return new Promise<void>((r) => (release = r));
  });
  reread();
  reread();
  reread();
  assert.equal(runs, 1);
  release();
  await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
  release();
  await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
});

test("a failed read does not stop the next one", async () => {
  let runs = 0;
  const reread = coalesce(() => {
    runs++;
    return Promise.reject(new Error("x"));
  });
  reread();
  await new Promise((r) => setImmediate(r));
  reread();
  await new Promise((r) => setImmediate(r));
  assert.equal(runs, 2);
});
