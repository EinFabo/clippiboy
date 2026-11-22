import { test } from "node:test";
import assert from "node:assert/strict";
import { ago, friendStatus, groupFriends, pendingCount, pendingRows } from "./friendsList.ts";
import type { FriendPending, FriendPresence, FriendUser, Transfer } from "./types.ts";

const user = (id: string, displayName: string): FriendUser => ({
  id,
  displayName,
  username: displayName.toLowerCase(),
  avatar: null,
});
const pending = (id: string, name: string, since: number): FriendPending => ({ ...user(id, name), since });
const on = (over: Partial<FriendPresence> = {}): FriendPresence => ({
  game: null,
  since: null,
  status: null,
  busy: false,
  ...over,
});
const transfer = (over: Partial<Transfer>): Transfer => ({
  id: "t",
  direction: "in",
  friendId: "1",
  friendName: "Matti",
  name: "Clutch 1v3",
  size: 100,
  moved: 0,
  stage: "asking",
  error: null,
  clipId: null,
  game: null,
  durationMs: 0,
  screenshot: false,
  recording: false,
  ...over,
});

test("status: busy over playing, nothing means offline", () => {
  assert.equal(friendStatus(undefined), "offline");
  assert.equal(friendStatus(on()), "online");
  assert.equal(friendStatus(on({ game: "Valorant" })), "playing");
  assert.equal(friendStatus(on({ busy: true })), "busy");
});

test("groups: in game, online with busy, offline by last seen; favourites first", () => {
  const friends = [user("a", "Ben"), user("b", "Jonas"), user("c", "Lea"), user("d", "Luca"), user("e", "Sophie"), user("f", "Tim")];
  const presence = { b: on(), d: on({ game: "CS2" }), e: on({ busy: true }) };
  const lastSeen = { a: 10, c: 30, f: 20 };
  const groups = groupFriends(friends, presence, lastSeen, ["e", "f"]);
  assert.deepEqual(groups.playing.map((f) => f.displayName), ["Luca"]);
  assert.deepEqual(groups.online.map((f) => f.displayName), ["Sophie", "Jonas"]);
  assert.deepEqual(groups.offline.map((f) => f.displayName), ["Tim", "Lea", "Ben"]);
});

test("pending: requests newest first, then clip offers, then one's own requests", () => {
  const rows = pendingRows(
    [pending("1", "Noah", 10), pending("2", "Ida", 20)],
    [pending("3", "Mia", 5)],
    [transfer({ id: "o" }), transfer({ id: "m", stage: "moving" }), transfer({ id: "x", direction: "out" })],
  );
  assert.deepEqual(
    rows.map((r) => (r.kind === "offer" ? `offer:${r.transfer.id}` : `${r.kind}:${r.request.displayName}`)),
    ["request:Ida", "request:Noah", "offer:o", "sent:Mia"],
  );
});

test("nav badge counts only what needs an answer", () => {
  const transfers = [
    transfer({ id: "a" }),
    transfer({ id: "b", stage: "moving" }),
    transfer({ id: "c", direction: "out", stage: "asking" }),
  ];
  assert.equal(pendingCount([pending("1", "Noah", 0)], transfers), 2);
  assert.equal(pendingCount([], []), 0);
});

test("ago", () => {
  const now = 10 * 86_400_000;
  assert.equal(ago(now - 20_000, now), "just now");
  assert.equal(ago(now - 20 * 60_000, now), "20 min ago");
  assert.equal(ago(now - 2 * 3_600_000, now), "2 h ago");
  assert.equal(ago(now - 30 * 3_600_000, now), "yesterday");
  assert.equal(ago(now - 5 * 86_400_000, now), "5 days ago");
  assert.equal(ago(now + 5_000, now), "just now");
});
