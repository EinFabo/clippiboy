import assert from "node:assert/strict";
import test from "node:test";
import { settleInPool } from "./pool.ts";

test("never more than the limit run at once, and every item gets its turn", async () => {
  let running = 0;
  let most = 0;
  const seen: number[] = [];
  const results = await settleInPool([1, 2, 3, 4, 5, 6, 7], 3, async (n) => {
    running++;
    most = Math.max(most, running);
    await new Promise((r) => setTimeout(r, 5 - (n % 3)));
    running--;
    seen.push(n);
    return n * 2;
  });
  assert.equal(most, 3);
  assert.deepEqual([...seen].sort(), [1, 2, 3, 4, 5, 6, 7]);
  assert.deepEqual(
    results.map((r) => (r.status === "fulfilled" ? r.value : null)),
    [2, 4, 6, 8, 10, 12, 14],
  );
});

test("a failure is kept in its place and does not stop the rest", async () => {
  const results = await settleInPool(["a", "b", "c"], 2, async (x) => {
    if (x === "b") throw new Error("refused");
    return x;
  });
  assert.equal(results[0].status, "fulfilled");
  assert.equal(results[1].status, "rejected");
  assert.equal((results[1] as PromiseRejectedResult).reason.message, "refused");
  assert.equal(results[2].status, "fulfilled");
});

test("nothing to do settles at once", async () => {
  assert.deepEqual(await settleInPool([], 4, async () => 1), []);
});
