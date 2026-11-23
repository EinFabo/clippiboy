import { test } from "node:test";
import assert from "node:assert/strict";
import { MAX_PENDING_OUT, NOBODY, requestRule, type RequestFacts } from "./friendrules.ts";

const base: RequestFacts = {
  me: "me",
  target: { id: "them", allowRequests: true },
  blockedByTarget: false,
  blockedByMe: false,
  existing: null,
  handedOut: true,
  pendingOut: 0,
};

test("a new pair gets a request", () => assert.deepEqual(requestRule(base), { kind: "send" }));

test("asking someone who asked first is a yes", () => {
  assert.deepEqual(requestRule({ ...base, existing: { status: "pending", requester: "them" } }), { kind: "accept" });
  // Even when they take no requests: the pair has a row already.
  assert.deepEqual(
    requestRule({ ...base, target: { id: "them", allowRequests: false }, existing: { status: "pending", requester: "them" } }),
    { kind: "accept" },
  );
});

test("friends, a request already sent, oneself — refused, nothing written", () => {
  assert.equal(requestRule({ ...base, existing: { status: "accepted", requester: "them" } }).kind, "refuse");
  assert.equal(requestRule({ ...base, existing: { status: "pending", requester: "me" } }).kind, "refuse");
  assert.equal(requestRule({ ...base, target: { id: "me", allowRequests: true } }).kind, "refuse");
});

test("blocks hold either way, and being blocked looks like nobody", () => {
  assert.deepEqual(requestRule({ ...base, blockedByTarget: true }), { kind: "refuse", status: 404, message: NOBODY });
  assert.equal(requestRule({ ...base, blockedByMe: true }).kind, "refuse");
});

test("someone who takes no requests says no to a code or invite, and is nobody by name", () => {
  const closed = { ...base, target: { id: "them", allowRequests: false } };
  assert.equal((requestRule(closed) as { status: number }).status, 403);
  assert.deepEqual(requestRule({ ...closed, handedOut: false }), { kind: "refuse", status: 404, message: NOBODY });
});

test("past the cap of waiting requests a new one is refused, saying yes still works", () => {
  const full = { ...base, pendingOut: MAX_PENDING_OUT };
  assert.deepEqual(requestRule({ ...base, pendingOut: MAX_PENDING_OUT - 1 }), { kind: "send" });
  assert.equal((requestRule(full) as { status: number }).status, 429);
  assert.deepEqual(requestRule({ ...full, existing: { status: "pending", requester: "them" } }), { kind: "accept" });
  // Says nothing new about whom they asked: nobody stays nobody, a block stays a block.
  assert.deepEqual(requestRule({ ...full, blockedByTarget: true }), { kind: "refuse", status: 404, message: NOBODY });
  assert.deepEqual(requestRule({ ...full, target: { id: "them", allowRequests: false }, handedOut: false }), {
    kind: "refuse",
    status: 404,
    message: NOBODY,
  });
});
