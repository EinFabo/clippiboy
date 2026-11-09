import { test } from "node:test";
import assert from "node:assert/strict";
import {
  clearInviteCookie,
  cookieValue,
  INVITE_COOKIE,
  INVITE_TTL_MS,
  inviteCookie,
  inviteTarget,
  ownClick,
  signInvite,
  verifyInvite,
} from "./invite.ts";

const SECRET = "test-secret";
const ALICE = "0b6f9c3e-1d2a-4c5b-8e7f-9a0b1c2d3e4f";
const BOB = "f4e3d2c1-b0a9-4f8e-9d7c-6b5a4f3e2d1c";
const now = 1_800_000_000_000;

test("a signed invite names who signed it", async () => {
  const token = await signInvite(SECRET, ALICE, now);
  assert.equal(await verifyInvite(SECRET, token, now), ALICE);
  assert.equal(await verifyInvite(SECRET, token, now + INVITE_TTL_MS - 1000), ALICE);
});

test("an invite runs out after its fourteen days", async () => {
  const token = await signInvite(SECRET, ALICE, now);
  assert.equal(await verifyInvite(SECRET, token, now + INVITE_TTL_MS + 1000), null);
});

test("another key, another id or a longer life breaks the signature", async () => {
  const token = await signInvite(SECRET, ALICE, now);
  assert.equal(await verifyInvite("other-secret", token, now), null);
  const [, expires, mac] = token.split(".");
  assert.equal(await verifyInvite(SECRET, `${BOB}.${expires}.${mac}`, now), null);
  const later = (parseInt(expires, 36) + 86_400).toString(36);
  assert.equal(await verifyInvite(SECRET, `${ALICE}.${later}.${mac}`, now), null);
});

test("garbage is no invite", async () => {
  for (const token of ["", "abc", "a.b.c", `${ALICE}.zz`, `${ALICE}.zz.!!!`, `${ALICE}.zz.a`, `not-a-uuid.zz.abcd`]) {
    assert.equal(await verifyInvite(SECRET, token, now), null, token);
  }
});

test("the cookie is read out of a header with others in it", async () => {
  const token = await signInvite(SECRET, ALICE, now);
  assert.equal(cookieValue(`cb_login=x.y; ${INVITE_COOKIE}=${token}; z=1`, INVITE_COOKIE), token);
  assert.equal(cookieValue("cb_login=x", INVITE_COOKIE), null);
  assert.equal(cookieValue(null, INVITE_COOKIE), null);
  assert.equal(cookieValue(`${INVITE_COOKIE}=`, INVITE_COOKIE), null);
});

test("the cookie covers the whole domain, so sign-in on the API sees it", () => {
  const set = inviteCookie("t");
  for (const part of ["Domain=.clippiboy.com", "Path=/", "HttpOnly", "Secure", "SameSite=Lax", `Max-Age=${INVITE_TTL_MS / 1000}`]) {
    assert.ok(set.includes(part), part);
  }
  assert.ok(clearInviteCookie().includes("Max-Age=0"));
  assert.ok(clearInviteCookie().includes("Domain=.clippiboy.com"));
});

test("only a click on our own page sets the cookie", () => {
  const origin = "https://clippiboy.com";
  assert.equal(ownClick("same-origin", null, origin), true);
  assert.equal(ownClick("cross-site", "https://clippiboy.com/i/ABCDEFGH", origin), false);
  assert.equal(ownClick("same-site", null, origin), false);
  assert.equal(ownClick("none", null, origin), false);
  // Browsers without Sec-Fetch-* fall back to the Referer.
  assert.equal(ownClick(null, "https://clippiboy.com/c/abc", origin), true);
  assert.equal(ownClick(null, "https://clippiboy.com.evil.example/", origin), false);
  assert.equal(ownClick(null, null, origin), false);
  assert.equal(ownClick(null, "not a url", origin), false);
});

test("an invite never asks the inviter to befriend themselves", () => {
  assert.equal(inviteTarget(ALICE, BOB), ALICE);
  assert.equal(inviteTarget(ALICE, ALICE), null);
  assert.equal(inviteTarget(null, BOB), null);
});
