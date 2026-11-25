import { test } from "node:test";
import assert from "node:assert/strict";
import { MISSED_MAX, MISSED_SHOWN_MS, isMissed, missedDetail, missedLine, noteMissed, shownMissed } from "./missedSends.ts";
import type { Transfer } from "./types.ts";

function transfer(id: string, stage: Transfer["stage"], extra: Partial<Transfer> = {}): Transfer {
  return {
    id,
    direction: "out",
    friendId: "f",
    friendName: "Matti",
    name: "Ace on Bind",
    size: 1,
    moved: 0,
    stage,
    error: null,
    clipId: "c",
    game: null,
    durationMs: 0,
    screenshot: false,
    recording: false,
    ...extra,
  };
}

test("only one's own sends that ended without getting there are missed", () => {
  for (const stage of ["declined", "expired", "failed"] as const) assert.ok(isMissed(transfer("a", stage)), stage);
  for (const stage of ["preparing", "asking", "moving", "done", "cancelled"] as const) {
    assert.ok(!isMissed(transfer("a", stage)), stage);
  }
  assert.ok(!isMissed(transfer("a", "failed", { direction: "in" })));
});

test("a no their app gave on its own is no one's", () => {
  assert.equal(missedLine(transfer("a", "declined", { refusal: "manual" }), "Matti"), "Matti said no");
  assert.equal(
    missedLine(transfer("a", "declined", { refusal: "auto", screenshot: true }), "Matti"),
    "Matti isn't taking screenshots right now",
  );
  // From an app before 1.0.2, which does not say.
  assert.equal(missedLine(transfer("a", "declined"), "A friend"), "A friend isn't taking clips right now");
  assert.equal(missedLine(transfer("a", "expired"), "Matti"), "Matti didn't answer");
  assert.equal(missedLine(transfer("a", "failed", { recording: true }), "Matti"), "Matti didn't get your recording");
});

test("a failure says why under the clip", () => {
  assert.equal(missedDetail(transfer("a", "failed", { error: "They went offline." })), "Ace on Bind · They went offline");
  assert.equal(missedDetail(transfer("a", "expired")), "Ace on Bind");
  assert.equal(missedDetail(transfer("a", "failed")), "Ace on Bind");
});

test("each is listed for a while after it was first seen, the newest first", () => {
  const list = [transfer("a", "expired"), transfer("b", "asking"), transfer("c", "failed")];
  const seen = noteMissed(list, {}, 1_000);
  assert.deepEqual(seen, { a: 1_000, c: 1_000 });
  // Nothing new: the same object, so nobody draws again.
  assert.equal(noteMissed(list, seen, 5_000), seen);
  const later = noteMissed([...list, transfer("d", "declined")], seen, 2_000);
  assert.deepEqual(
    shownMissed([...list, transfer("d", "declined")], later, 3_000).map((t) => t.id),
    ["d", "a", "c"],
  );
  assert.deepEqual(
    shownMissed(list, later, 1_000 + MISSED_SHOWN_MS).map((t) => t.id),
    [],
  );
  // Gone from the core's list: forgotten.
  assert.deepEqual(noteMissed([transfer("a", "expired")], later, 4_000), { a: 1_000 });
});

test("the console lists a few at most", () => {
  const list = Array.from({ length: MISSED_MAX + 2 }, (_, i) => transfer(`t${i}`, "expired"));
  let seen: Record<string, number> = {};
  list.forEach((t, i) => (seen = noteMissed(list.slice(0, i + 1), seen, i)));
  assert.equal(shownMissed(list, seen, 10).length, MISSED_MAX);
  assert.equal(shownMissed(list, seen, 10)[0].id, `t${MISSED_MAX + 1}`);
});
