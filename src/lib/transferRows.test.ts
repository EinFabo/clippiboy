import { test } from "node:test";
import assert from "node:assert/strict";
import {
  CARD_ENDED_MS,
  cardLeft,
  noteEnded,
  onFriendsPage,
  presenceSig,
  reason,
  rowTransfer,
  toDismiss,
  transferKind,
  type Ended,
} from "./transferRows.ts";
import type { FriendPresence, Transfer } from "./types.ts";

const t = (over: Partial<Transfer>): Transfer => ({
  id: "t",
  direction: "out",
  friendId: "1",
  friendName: "Luca",
  name: "Ace on Mirage",
  size: 100,
  moved: 0,
  stage: "moving",
  error: null,
  clipId: "clip",
  game: null,
  durationMs: 0,
  screenshot: false,
  recording: false,
  ...over,
});
const on = (over: Partial<FriendPresence> = {}): FriendPresence => ({
  game: null,
  since: null,
  status: null,
  busy: false,
  ...over,
});

test("kinds: offers, running, failures, endings, gone", () => {
  assert.equal(transferKind(t({ direction: "in", stage: "asking" })), "offer");
  assert.equal(transferKind(t({ stage: "asking" })), "active");
  assert.equal(transferKind(t({ stage: "preparing" })), "active");
  assert.equal(transferKind(t({ direction: "in", stage: "moving" })), "active");
  assert.equal(transferKind(t({ stage: "failed" })), "error");
  assert.equal(transferKind(t({ stage: "expired" })), "error");
  assert.equal(transferKind(t({ direction: "in", stage: "failed" })), "error");
  assert.equal(transferKind(t({ stage: "done" })), "ended");
  assert.equal(transferKind(t({ stage: "declined" })), "ended");
  assert.equal(transferKind(t({ direction: "in", stage: "done" })), "ended");
  assert.equal(transferKind(t({ direction: "in", stage: "expired" })), "ended");
  assert.equal(transferKind(t({ direction: "in", stage: "declined" })), "gone");
  assert.equal(transferKind(t({ stage: "cancelled" })), "gone");
});

test("presence sig: the game, busy and the status line count, the start time doesn't", () => {
  assert.equal(presenceSig(undefined), "offline");
  assert.equal(presenceSig(on({ game: "CS2", since: 1 })), presenceSig(on({ game: "CS2", since: 2 })));
  assert.notEqual(presenceSig(on()), presenceSig(on({ game: "CS2" })));
  assert.notEqual(presenceSig(on()), presenceSig(on({ busy: true })));
  assert.notEqual(presenceSig(on()), presenceSig(on({ status: "afk" })));
});

test("ended: noted once with the friend's state, forgotten when gone from the list", () => {
  const presence = { "1": on({ game: "CS2" }) };
  const running = [t({ id: "a" })];
  const empty: Record<string, Ended> = {};
  assert.equal(noteEnded(running, empty, presence, 5), empty);
  const done = [t({ id: "a", stage: "done" })];
  const first = noteEnded(done, empty, presence, 5);
  assert.deepEqual(first, { a: { at: 5, sig: presenceSig(presence["1"]) } });
  assert.equal(noteEnded(done, first, {}, 9), first, "the moment stays");
  assert.deepEqual(noteEnded([], first, presence, 9), {});
});

test("dismiss: gone at once, ended when the friend moved on, failures never by themselves", () => {
  const presence = { "1": on({ game: "CS2" }) };
  const transfers = [
    t({ id: "done", stage: "done" }),
    t({ id: "fail", stage: "failed" }),
    t({ id: "cancel", stage: "cancelled" }),
    t({ id: "run" }),
  ];
  const ended = noteEnded(transfers, {}, presence, 0);
  assert.deepEqual(toDismiss(transfers, ended, presence), ["cancel"]);
  assert.deepEqual(toDismiss(transfers, ended, { "1": on({ game: "CS2", since: 99 }) }), ["cancel"]);
  assert.deepEqual(toDismiss(transfers, ended, { "1": on() }), ["done", "cancel"]);
  assert.deepEqual(toDismiss(transfers, ended, {}), ["done", "cancel"]);
});

test("cards: running and failed stay, ended ones for a few seconds, gone never", () => {
  const ended = { d: { at: 1000, sig: "" } };
  assert.equal(cardLeft(t({}), ended, 5000), Infinity);
  assert.equal(cardLeft(t({ stage: "failed" }), ended, 1e9), Infinity);
  assert.equal(cardLeft(t({ id: "d", stage: "done" }), ended, 3000), CARD_ENDED_MS - 2000);
  assert.equal(cardLeft(t({ id: "d", stage: "done" }), ended, 1e9), 0);
  assert.equal(cardLeft(t({ id: "new", stage: "done" }), ended, 1e9), CARD_ENDED_MS);
  assert.equal(cardLeft(t({ stage: "cancelled" }), ended, 0), 0);
});

test("the Friends page shows offers and transfers of friends on the list itself", () => {
  const friends = new Set(["1"]);
  assert.equal(onFriendsPage(t({}), friends), true);
  assert.equal(onFriendsPage(t({ friendId: "9" }), friends), false);
  assert.equal(onFriendsPage(t({ friendId: "9", direction: "in", stage: "asking" }), friends), true);
});

test("row: sending with its share, waiting without one, several at once", () => {
  assert.deepEqual(rowTransfer("1", [t({ moved: 60 })], {}), {
    kind: "sending",
    ids: ["t"],
    text: "Sending Ace on Mirage",
    share: 0.6,
  });
  assert.equal(rowTransfer("1", [t({ stage: "asking" })], {})?.text, "Waiting for an answer · Ace on Mirage");
  assert.equal(rowTransfer("1", [t({ stage: "asking" })], {})?.kind === "sending", true);
  const two = rowTransfer("1", [t({ id: "a", moved: 100 }), t({ id: "b", stage: "asking", size: 300 })], {});
  assert.deepEqual(two, { kind: "sending", ids: ["a", "b"], text: "Sending 2 clips", share: 0.25 });
  const shots = rowTransfer("1", [t({ id: "a", screenshot: true }), t({ id: "b", screenshot: true })], {});
  assert.equal(shots?.text, "Sending 2 screenshots");
  assert.equal(rowTransfer("2", [t({})], {}), null);
});

test("row: receiving in the sender's row; an offer is not the row's", () => {
  const r = rowTransfer("1", [t({ direction: "in", moved: 50 })], {});
  assert.deepEqual(r, { kind: "receiving", ids: ["t"], text: "Receiving Ace on Mirage", share: 0.5 });
  assert.equal(rowTransfer("1", [t({ direction: "in", stage: "asking" })], {}), null);
});

test("row: a failure with Retry for what one sent", () => {
  const failed = t({ id: "f", stage: "failed", error: "The connection dropped." });
  const r = rowTransfer("1", [failed], {});
  assert.equal(r?.kind, "error");
  assert.equal(r?.text, "Couldn't send Ace on Mirage · the connection dropped");
  assert.deepEqual(r?.kind === "error" && r.retry.map((x) => x.id), ["f"]);
  const incoming = rowTransfer("1", [t({ direction: "in", stage: "failed", clipId: null })], {});
  assert.equal(incoming?.text, "Ace on Mirage didn't arrive · it didn't go through");
  assert.deepEqual(incoming?.kind === "error" && incoming.retry, []);
  const two = rowTransfer("1", [failed, t({ id: "g", stage: "expired" })], {});
  assert.equal(two?.text, "Couldn't send 2 clips");
  assert.equal(two?.kind === "error" && two.retry.length, 2);
  // Running wins over a failure; the failure comes back after.
  assert.equal(rowTransfer("1", [failed, t({ id: "r" })], {})?.kind, "sending");
});

test("row: ended — the newest, green for arrived, Open for a received clip", () => {
  const ended = { a: { at: 10, sig: "" }, b: { at: 20, sig: "" }, c: { at: 5, sig: "" } };
  const r = rowTransfer(
    "1",
    [t({ id: "a", stage: "done", name: "Scoreboard" }), t({ id: "b", stage: "done", name: "Ace" }), t({ id: "c", stage: "declined" })],
    ended,
  );
  assert.deepEqual(r, {
    kind: "done",
    ids: ["b", "a", "c"],
    text: "Has your Ace and 1 more",
    tone: "ok",
    at: 20,
    open: null,
  });
  const got = rowTransfer("1", [t({ id: "a", direction: "in", stage: "done", clipId: "mine" })], ended);
  assert.equal(got?.text, "Sent you Ace on Mirage");
  assert.equal(got?.kind === "done" && got.open, "mine");
  const no = rowTransfer("1", [t({ id: "a", stage: "declined" })], ended);
  assert.equal(no?.kind === "done" && `${no.text}|${no.tone}`, "Said no to Ace on Mirage|muted");
  assert.equal(rowTransfer("1", [t({ id: "a", direction: "in", stage: "expired" })], ended)?.text, "Missed Ace on Mirage");
});

test("reason reads on after a dot", () => {
  assert.equal(reason(t({ stage: "failed", error: "They are busy right now." })), "they are busy right now");
  assert.equal(reason(t({ stage: "failed", error: "ClippiBoy could not read the file." })), "ClippiBoy could not read the file");
  assert.equal(reason(t({ stage: "expired" })), "no answer");
});

test("row: clips still queued for the friend count in", () => {
  const queued = [{ name: "Retake B", screenshot: false, recording: false }];
  // Handed over, nothing in the list yet.
  assert.deepEqual(rowTransfer("1", [], {}, queued), { kind: "sending", ids: [], text: "Getting Retake B ready", share: null });
  assert.equal(rowTransfer("1", [], {}, [...queued, ...queued])?.text, "Getting Retake B ready · 1 more");
  // One waiting for an answer, one behind it: still the first's answer.
  assert.equal(rowTransfer("1", [t({ stage: "asking" })], {}, queued)?.text, "Waiting for an answer · Ace on Mirage · 1 more");
  // Once one moves it is all one sending.
  assert.equal(rowTransfer("1", [t({ moved: 10 }), t({ id: "b", stage: "asking" })], {}, queued)?.text, "Sending 3 clips");
  const shot = [{ name: "Scoreboard", screenshot: true, recording: false }];
  assert.equal(rowTransfer("1", [t({ moved: 50 })], {}, shot)?.text, "Sending 2 clips");
  assert.equal(rowTransfer("1", [t({ moved: 50 })], {}, shot)?.kind === "sending" && rowTransfer("1", [t({ moved: 50 })], {}, shot)?.share, 0.5);
  // Receiving from them while one's own clips wait: the sending shows.
  assert.equal(rowTransfer("1", [t({ direction: "in", moved: 10 })], {}, queued)?.kind, "sending");
});
