import { test } from "node:test";
import assert from "node:assert/strict";
import {
  addAnswer,
  awayOrder,
  maskedCode,
  myStatus,
  newcomer,
  nextDrop,
  seenWhileAway,
  shortLink,
  signInFailed,
  statusChoice,
  statusPatch,
  statusText,
} from "./friendsPage.ts";
import type { FriendUser } from "./types.ts";

const user = (id: string, displayName: string): FriendUser => ({
  id,
  displayName,
  username: displayName.toLowerCase(),
  avatar: null,
});

test("status chip: connecting first, then appear offline over busy", () => {
  assert.equal(myStatus({ busy: false, invisible: false }, true), "online");
  assert.equal(myStatus({ busy: true, invisible: false }, true), "busy");
  assert.equal(myStatus({ busy: true, invisible: true }, true), "invisible");
  assert.equal(myStatus({ busy: false, invisible: true }, false), "connecting");
  assert.equal(statusChoice({ busy: true, invisible: true }), "invisible");
  assert.equal(statusChoice({ busy: true, invisible: false }), "busy");
});

test("status menu: a pick turns on at most one of busy and invisible", () => {
  assert.deepEqual(statusPatch("online"), { busy: false, invisible: false });
  assert.deepEqual(statusPatch("busy"), { busy: true, invisible: false });
  assert.deepEqual(statusPatch("invisible"), { busy: false, invisible: true });
  // The text typed in the menu goes along with the pick, cleaned.
  assert.deepEqual(statusPatch("busy", "  back at 8 "), { busy: true, invisible: false, status: "back at 8" });
  assert.equal(statusText("x".repeat(70)).length, 60);
});

test("add answer: sent and accepted name the friend once known", () => {
  assert.deepEqual(addAnswer({ ok: true, status: "sent" }, "Noah"), { tone: "ok", text: "Request sent to Noah." });
  assert.deepEqual(addAnswer({ ok: true, status: "sent" }, null), { tone: "ok", text: "Request sent." });
  assert.equal(addAnswer({ ok: true, status: "accepted" }, "Noah").text, "Noah had asked you already. You are friends now.");
  assert.equal(addAnswer({ ok: true, status: "accepted" }, null).tone, "ok");
});

test("add answer: nothing to do is calm, refusals are red in the server's words", () => {
  assert.deepEqual(addAnswer({ ok: false, error: "You are already friends." }, null), {
    tone: "info",
    text: "You are already friends.",
  });
  assert.equal(addAnswer({ ok: false, error: "Request already sent." }, null).tone, "info");
  assert.deepEqual(addAnswer({ ok: false, error: "Nobody with that name or code uses ClippiBoy." }, null), {
    tone: "bad",
    text: "Nobody with that name or code uses ClippiBoy.",
  });
  assert.equal(addAnswer({ ok: false, error: "They don't take friend requests right now." }, null).tone, "bad");
  assert.equal(addAnswer({ ok: false, error: "  " }, null).text, "Something went wrong.");
});

test("newcomer is the first user not seen before", () => {
  const before = new Set(["1", "2"]);
  assert.equal(newcomer(before, [user("1", "A"), user("2", "B")]), null);
  assert.equal(newcomer(before, [user("1", "A"), user("3", "Noah")])?.displayName, "Noah");
});

test("code and link as the screen shows them", () => {
  assert.equal(shortLink("K7Q2M9XD"), "clippiboy.com/i/K7Q2M9XD");
  assert.equal(maskedCode("K7Q2M9XD"), "••••-••••");
  assert.equal(maskedCode("ABC"), "•••");
});

test("a drop remembers who was online until then; reconnecting forgets", () => {
  const online = { connected: true, presence: { "1": {}, "2": {} } };
  const away = { connected: false, presence: {} };
  const drop = nextDrop(online, away, 1000, null);
  assert.deepEqual(drop, { at: 1000, online: ["1", "2"] });
  // Still away: the first drop stays, later views have no presence to tell.
  assert.equal(nextDrop(away, away, 5000, drop), drop);
  assert.equal(nextDrop(away, { connected: true }, 6000, drop), null);
  // Never connected since the start: nothing known.
  assert.equal(nextDrop(away, away, 7000, null), null);
});

test("while away: online at the drop counts as seen then, the rest as the hub said", () => {
  const drop = { at: 9000, online: ["1"] };
  const lastSeen = { "2": 4000 };
  assert.equal(seenWhileAway("1", lastSeen, drop), 9000);
  assert.equal(seenWhileAway("2", lastSeen, drop), 4000);
  assert.equal(seenWhileAway("3", lastSeen, drop), undefined);
  assert.equal(seenWhileAway("1", lastSeen, null), undefined);
  const order = awayOrder([user("3", "Cleo"), user("2", "Ben"), user("4", "Abe"), user("1", "Zed")], lastSeen, drop);
  assert.deepEqual(order.map((u) => u.displayName), ["Zed", "Ben", "Abe", "Cleo"]);
});

test("sign-in failed only when waiting ends signed out and not by Cancel", () => {
  const waiting = { signingIn: true };
  assert.equal(signInFailed(waiting, { signingIn: false, signedIn: false }, false), true);
  assert.equal(signInFailed(waiting, { signingIn: false, signedIn: false }, true), false);
  assert.equal(signInFailed(waiting, { signingIn: false, signedIn: true }, false), false);
  assert.equal(signInFailed({ signingIn: false }, { signingIn: false, signedIn: false }, false), false);
});
