import { test } from "node:test";
import assert from "node:assert/strict";
import { LOGIN_TTL_MS, pageCsp, signState, statesIn, verifyState, withoutState, withState } from "./login.ts";

test("a second sign-in start keeps the first tab's state", () => {
  const first = withState(statesIn("x=1", "cb_login"), "aaa");
  const both = withState(statesIn(`x=1; cb_login=${first}`, "cb_login"), "bbb");
  assert.deepEqual(statesIn(`cb_login=${both}`, "cb_login"), ["aaa", "bbb"]);
});
test("only the last three starts are kept", () => assert.equal(withState(["a", "b", "c"], "d"), "b.c.d"));
test("no cookie, or someone else's, holds no state", () => {
  assert.deepEqual(statesIn(null, "cb_login"), []);
  assert.deepEqual(statesIn("other=abc", "cb_login"), []);
});
test("the sign-in pages run only their own style and script, and refuse frames", () => {
  const csp = pageCsp("n0nce");
  assert.match(csp, /^default-src 'none'/);
  assert.match(csp, /script-src 'nonce-n0nce'(;|$)/);
  assert.match(csp, /style-src 'nonce-n0nce'(;|$)/);
  assert.match(csp, /frame-ancestors 'none'/);
  assert.doesNotMatch(csp, /unsafe-inline/);
});
test("a form goes only back to this server, and only on the page that has one", () => {
  assert.match(pageCsp("n"), /form-action 'none'/);
  assert.match(pageCsp("n", true), /form-action 'self'(;|$)/);
  assert.doesNotMatch(pageCsp("n", true), /form-action 'none'/);
});

const secret = "discord-client-secret";
const challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const now = Date.UTC(2026, 10, 25, 12, 0, 0);

test("a state carries the challenge and when the sign-in started, and nothing has to be written for it", async () => {
  const state = await signState(secret, challenge, now);
  assert.deepEqual(await verifyState(secret, state, now + 1000), { challenge, issuedAt: now });
  // base64url only: it travels through Discord and sits in the cookie beside two others.
  assert.match(state, /^[A-Za-z0-9_-]+$/);
  const cookie = withState(["older"], state);
  assert.deepEqual(statesIn(`cb_login=${cookie}`, "cb_login"), ["older", state]);
});

test("two starts of the same sign-in at the same moment get different states", async () => {
  assert.notEqual(await signState(secret, challenge, now), await signState(secret, challenge, now));
});

test("a state is good for the sign-in's ten minutes, and not from far ahead of the clock", async () => {
  const state = await signState(secret, challenge, now);
  assert.notEqual(await verifyState(secret, state, now + LOGIN_TTL_MS), null);
  assert.equal(await verifyState(secret, state, now + LOGIN_TTL_MS + 1), null);
  assert.equal(await verifyState(secret, state, now - 5 * 60 * 1000), null);
});

test("a forged, changed or foreign state is no state", async () => {
  const state = await signState(secret, challenge, now);
  assert.equal(await verifyState("another secret", state, now), null);
  // Another challenge in a state of ours: the app that knows its verifier must not be swapped out.
  const other = "A".repeat(43);
  assert.equal(await verifyState(secret, state.slice(0, 32) + other + state.slice(75), now), null);
  // Another start time, so an old state cannot be made young again.
  const younger = await signState(secret, challenge, now + 60_000);
  assert.equal(await verifyState(secret, younger.slice(0, 32) + state.slice(32), now + 60_000), null);
  // A second spelling of the same MAC (the last character's spare bits).
  const last = state.at(-1)!;
  const respelled = state.slice(0, -1) + (last === "A" ? "B" : "A");
  assert.equal(await verifyState(secret, respelled, now), null);
  // The random states from before, junk, and nothing.
  for (const junk of ["x".repeat(32), "", state + "A", state.slice(1), `${state.slice(0, 117)}.`]) {
    assert.equal(await verifyState(secret, junk, now), null, junk);
  }
});

test("a spent state leaves the cookie, the browser's other starts stay", () => {
  assert.equal(withoutState(["a", "b", "c"], "b"), "a.c");
  assert.equal(withoutState(["a"], "a"), "");
  assert.equal(withoutState(["a", "b"], "z"), "a.b");
});
