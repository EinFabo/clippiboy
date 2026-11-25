// The bucket's SQL (`LIVE_BYTES`, `HOLD_CLIP` in shares.ts) run on SQLite,
// the engine under D1, with the migrations as D1 applies them.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { HOLD_CLIP, LIVE_BYTES, PENDING_MS } from "./shares.ts";

const MIGRATIONS = new URL("../migrations/", import.meta.url);
const NOW = 1_800_000_000_000;
const OWNER = "00000000-0000-4000-8000-00000000000a";

function database(): DatabaseSync {
  const db = new DatabaseSync(":memory:");
  // As D1 does.
  db.exec("PRAGMA foreign_keys = ON");
  for (const file of readdirSync(MIGRATIONS).filter((name) => name.endsWith(".sql")).sort()) {
    db.exec(readFileSync(new URL(file, MIGRATIONS), "utf8"));
  }
  db.prepare(
    `INSERT INTO users (id, discord_id, username, display_name, avatar, friend_code, created_at)
     VALUES (?, 'test-a', 'testalpha', 'Test Alpha', NULL, 'ABCDEFGH', ?)`,
  ).run(OWNER, NOW);
  return db;
}

function share(db: DatabaseSync, id: string, bytes: number, fields: { pending?: number; createdAt?: number; deletedAt?: number } = {}) {
  const createdAt = fields.createdAt ?? NOW - 60_000;
  db.prepare(
    `INSERT INTO shares (id, owner, bytes, title, game, width, height, created_at, expires_at, tags, show_name, offer_friend, pending, deleted_at)
     VALUES (?, ?, ?, 'Clip', NULL, 1920, 1080, ?, ?, '[]', 0, 0, ?, ?)`,
  ).run(id, OWNER, bytes, createdAt, createdAt + 5 * 24 * 60 * 60 * 1000, fields.pending ?? 0, fields.deletedAt ?? null);
}

const bucket = (db: DatabaseSync) =>
  (db.prepare(`SELECT (${LIVE_BYTES}) AS total`).get(NOW - PENDING_MS) as { total: number }).total;
const hold = (db: DatabaseSync, id: string, at = NOW) => db.prepare(HOLD_CLIP).run(id, at);
const held = (db: DatabaseSync, id: string) =>
  db.prepare("SELECT held_at, bytes FROM held_clips WHERE share_id = ?").get(id) as { held_at: number; bytes: number } | undefined;

test("the bucket holds live links and uploads in time to finish, not deleted or given-up ones", () => {
  const db = database();
  assert.equal(bucket(db), 0);
  share(db, "live", 100);
  share(db, "uploading", 50, { pending: 1 });
  share(db, "brokeoff", 25, { pending: 1, createdAt: NOW - PENDING_MS - 1 });
  share(db, "deleted", 1000, { deletedAt: NOW });
  assert.equal(bucket(db), 150);
});

test("a clip held for a report counts against the bucket after its link has gone", () => {
  const db = database();
  share(db, "reported", 1000);
  share(db, "other", 100);
  // As `deleteShare` does it: held first, then the link's row deleted.
  hold(db, "reported");
  db.prepare("UPDATE shares SET deleted_at = ? WHERE id = 'reported'").run(NOW);
  assert.deepEqual({ ...held(db, "reported") }, { held_at: NOW, bytes: 1000 });
  assert.equal(bucket(db), 1100);
  // Its account deleted: the rows cascade away, the held clip still counts.
  hold(db, "other");
  db.prepare("DELETE FROM users WHERE id = ?").run(OWNER);
  assert.equal((db.prepare("SELECT COUNT(*) AS n FROM shares").get() as { n: number }).n, 0);
  assert.equal(bucket(db), 1100);
  // Let go by the sweep: it counts no more.
  db.prepare("DELETE FROM held_clips WHERE share_id = 'reported'").run();
  assert.equal(bucket(db), 100);
});

test("a clip is held once, and one without its row is still held", () => {
  const db = database();
  share(db, "reported", 1000);
  hold(db, "reported", NOW);
  hold(db, "reported", NOW + 1);
  assert.deepEqual({ ...held(db, "reported") }, { held_at: NOW, bytes: 1000 });
  hold(db, "norow");
  assert.deepEqual({ ...held(db, "norow") }, { held_at: NOW, bytes: 0 });
});
