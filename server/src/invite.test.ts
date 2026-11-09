import { test } from "node:test";
import assert from "node:assert/strict";
import {
  clearInviteCookie,
  cookieValue,
  INVITE_COOKIE,
  INVITE_TTL_MS,
  ISSUE_SLACK_MS,
  inviteCookie,
  inviteKey,
  inviteOpen,
  inviteTarget,
  ownClick,
  signInvite,
  verifyInvite,
} from "./invite.ts";

const SECRET = "test-secret";
const ALICE = "0b6f9c3e-1d2a-4c5b-8e7f-9a0b1c2d3e4f";
const BOB = "f4e3d2c1-b0a9-4f8e-9d7c-6b5a4f3e2d1c";
const now = 1_800_000_000_000;

test("a signed invite names who signed it and when", async () => {
  const token = await signInvite(SECRET, ALICE, now);
  const invite = await verifyInvite(SECRET, token, now);
  assert.equal(invite?.inviter, ALICE);
  // The expiry is kept in whole seconds.
  assert.ok(invite && invite.issuedAt <= now && invite.issuedAt > now - 1000);
  assert.equal((await verifyInvite(SECRET, token, now + INVITE_TTL_MS - 1000))?.inviter, ALICE);
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

test("only a click on our own page, on clippiboy.com itself, sets the cookie", () => {
  const origin = "https://clippiboy.com";
  const url = "https://clippiboy.com/i/ABCDEFGH/add";
  assert.equal(ownClick(url, "same-origin", null, origin), true);
  assert.equal(ownClick(url, "cross-site", "https://clippiboy.com/i/ABCDEFGH", origin), false);
  assert.equal(ownClick(url, "same-site", null, origin), false);
  assert.equal(ownClick(url, "none", null, origin), false);
  // Browsers without Sec-Fetch-* fall back to the Referer.
  assert.equal(ownClick(url, null, "https://clippiboy.com/c/abc", origin), true);
  assert.equal(ownClick(url, null, "https://clippiboy.com.evil.example/", origin), false);
  assert.equal(ownClick(url, null, null, origin), false);
  assert.equal(ownClick(url, null, "not a url", origin), false);
  // A same-origin click on the API's own address is no click on our page.
  assert.equal(ownClick("https://api.clippiboy.com/i/ABCDEFGH/add", "same-origin", null, origin), false);
  assert.equal(ownClick("https://clippiboy-friends.x.workers.dev/c/abc/add", "same-origin", null, origin), false);
});

test("an invite asks only from an account made after the click", () => {
  const invite = { inviter: ALICE, issuedAt: now };
  const ok = { banned: false };
  assert.equal(inviteTarget(invite, { id: BOB, createdAt: now + 5 * 60_000 }, ok), ALICE);
  assert.equal(inviteTarget(invite, { id: BOB, createdAt: now - ISSUE_SLACK_MS + 1 }, ok), ALICE);
  // An account from before — signing in again, or someone else in the same browser.
  assert.equal(inviteTarget(invite, { id: BOB, createdAt: now - ISSUE_SLACK_MS - 1 }, ok), null);
  assert.equal(inviteTarget(invite, { id: BOB, createdAt: 0 }, ok), null);
});

test("no invite, oneself, a gone or suspended inviter — nobody is asked", () => {
  const invite = { inviter: ALICE, issuedAt: now };
  const fresh = { id: BOB, createdAt: now };
  assert.equal(inviteTarget(null, fresh, { banned: false }), null);
  assert.equal(inviteTarget(invite, { id: ALICE, createdAt: now }, { banned: false }), null);
  assert.equal(inviteTarget(invite, fresh, null), null);
  assert.equal(inviteTarget(invite, fresh, { banned: true }), null);
});

test("without any secret there are no invites, not an empty key", () => {
  assert.equal(inviteKey(undefined, undefined), null);
  assert.equal(inviteKey("", ""), null);
  assert.equal(inviteKey(undefined, "discord"), "discord");
  assert.equal(inviteKey("invite", "discord"), "invite");
});

test("an invite link works only for someone who takes requests and is not suspended", () => {
  assert.equal(inviteOpen({ allowRequests: true, banned: false }, true), true);
  assert.equal(inviteOpen({ allowRequests: false, banned: false }, true), false);
  assert.equal(inviteOpen({ allowRequests: true, banned: true }, true), false);
  assert.equal(inviteOpen(null, true), false);
  assert.equal(inviteOpen({ allowRequests: true, banned: false }, false), false);
});
