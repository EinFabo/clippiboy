import { friendCode, HttpError, sha256, base64url } from "./util";
import { carryOver, KEPT_LINKS, type DepartedRow } from "./departed";
import { newId, PENDING_MS, WEEK_MS } from "./shares";

export interface User {
  id: string;
  username: string;
  displayName: string;
  avatar: string | null;
}

export interface Me extends User {
  friendCode: string;
  allowRequests: boolean;
}

interface UserRow {
  id: string;
  username: string;
  display_name: string;
  avatar: string | null;
  friend_code: string;
  allow_requests: number;
}

export function toUser(row: UserRow): User {
  return { id: row.id, username: row.username, displayName: row.display_name, avatar: row.avatar };
}

export function toMe(row: UserRow): Me {
  return { ...toUser(row), friendCode: row.friend_code, allowRequests: row.allow_requests === 1 };
}

/** A friendship row is keyed by the pair in order. */
export function pair(a: string, b: string): [string, string] {
  return a < b ? [a, b] : [b, a];
}

export async function userById(db: D1Database, id: string): Promise<UserRow | null> {
  return db.prepare("SELECT * FROM users WHERE id = ?").bind(id).first<UserRow>();
}

export async function userByCode(db: D1Database, code: string): Promise<UserRow | null> {
  return db.prepare("SELECT * FROM users WHERE friend_code = ?").bind(code).first<UserRow>();
}

export async function userByName(db: D1Database, name: string): Promise<UserRow | null> {
  return db.prepare("SELECT * FROM users WHERE username = ? AND username != ''").bind(name).first<UserRow>();
}

/** Everyone the user has a row with, whatever its status. */
export async function relatedIds(db: D1Database, me: string): Promise<string[]> {
  const { results } = await db
    .prepare(
      "SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END AS id FROM friendships WHERE user_a = ?1 OR user_b = ?1",
    )
    .bind(me)
    .all<{ id: string }>();
  return results.map((row) => row.id);
}

/** Accepted friends only — the ones who may see the user's presence. */
export async function friendIds(db: D1Database, me: string): Promise<string[]> {
  const { results } = await db
    .prepare(
      "SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END AS id FROM friendships WHERE (user_a = ?1 OR user_b = ?1) AND status = 'accepted'",
    )
    .bind(me)
    .all<{ id: string }>();
  return results.map((row) => row.id);
}

export async function areFriends(db: D1Database, a: string, b: string): Promise<boolean> {
  const [first, second] = pair(a, b);
  const row = await db
    .prepare("SELECT 1 FROM friendships WHERE user_a = ? AND user_b = ? AND status = 'accepted'")
    .bind(first, second)
    .first();
  return row !== null;
}

export interface DiscordUser {
  id: string;
  username: string;
  global_name: string | null;
  avatar: string | null;
}

function avatarUrl(user: DiscordUser): string {
  if (user.avatar) return `https://cdn.discordapp.com/avatars/${user.id}/${user.avatar}.png?size=128`;
  return `https://cdn.discordapp.com/embed/avatars/${Number((BigInt(user.id) >> 22n) % 6n)}.png`;
}

/**
 * Creates the account on the first login and keeps name and avatar in step
 * after. `departed` is the fingerprint of the Discord id (null without the
 * pepper): a new account takes over what an account deleted within the week
 * left behind — its links still count against the week, its suspension holds.
 */
export async function upsertDiscordUser(db: D1Database, discord: DiscordUser, departed: string | null): Promise<string> {
  const displayName = discord.global_name || discord.username;
  const avatar = avatarUrl(discord);
  // Discord hands a released name to someone else: the stale row gives it up first,
  // since a name belongs to one account.
  const releaseName = db
    .prepare("UPDATE users SET username = '' WHERE username = ? AND username != '' AND discord_id != ?")
    .bind(discord.username, discord.id);
  const existing = await db
    .prepare("SELECT id FROM users WHERE discord_id = ?")
    .bind(discord.id)
    .first<{ id: string }>();
  if (existing) {
    await db.batch([
      releaseName,
      db
        .prepare("UPDATE users SET username = ?, display_name = ?, avatar = ? WHERE id = ?")
        .bind(discord.username, displayName, avatar, existing.id),
    ]);
    return existing.id;
  }
  const id = crypto.randomUUID();
  const now = Date.now();
  const left = departed
    ? await db.prepare("SELECT link_times, banned_at, left_at FROM departed WHERE hash = ?").bind(departed).first<DepartedRow>()
    : null;
  const carried = carryOver(left, now);
  // Only once the account is in: each statement checks for it, since a
  // friend-code collision leaves the insert undone.
  const created = "EXISTS (SELECT 1 FROM users WHERE id = ?)";
  const takeOver = [
    // Each counting link comes back as a deleted one — a deleted link counts
    // against the week like any other, and the sweep forgets it after two.
    ...(carried?.linkTimes ?? []).map((at) =>
      db
        .prepare(
          `INSERT INTO shares (id, owner, bytes, title, created_at, expires_at, deleted_at) SELECT ?, ?, 0, '', ?, ?, ? WHERE ${created}`,
        )
        .bind(newId(), id, at, at, now, id),
    ),
    ...(carried?.bannedAt != null
      ? [db.prepare("UPDATE users SET banned_at = ? WHERE id = ?").bind(carried.bannedAt, id)]
      : []),
    // Taken over, or past its week: either way the row is done with.
    ...(left ? [db.prepare(`DELETE FROM departed WHERE hash = ? AND ${created}`).bind(departed, id)] : []),
  ];
  // A collision among 31^8 codes is unlikely, not impossible.
  for (let attempt = 0; attempt < 5; attempt++) {
    const results = await db.batch([
      releaseName,
      db
        .prepare(
          "INSERT INTO users (id, discord_id, username, display_name, avatar, friend_code, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (friend_code) DO NOTHING",
        )
        .bind(id, discord.id, discord.username, displayName, avatar, friendCode(), now),
      ...takeOver,
    ]);
    if (results[1].meta.changes === 1) return id;
  }
  throw new HttpError(500, "Could not create the account.");
}

/**
 * The statement that, run in one batch before the account's row is deleted,
 * leaves behind what must outlive it: the times of its links that still count
 * against the week (the newest few) and its suspension, under the fingerprint
 * of its Discord id. Nothing when there is neither.
 */
export function departStatement(db: D1Database, me: string, departed: string, now: number): D1PreparedStatement {
  return db
    .prepare(
      `INSERT INTO departed (hash, link_times, banned_at, left_at)
       SELECT ?1, links, banned_at, ?2 FROM (
         SELECT u.banned_at, (
           SELECT json_group_array(created_at) FROM (
             SELECT s.created_at FROM shares s JOIN users o ON o.id = s.owner
              WHERE s.owner = ?3 AND s.created_at > MAX(?4, o.links_reset_at) AND (s.pending = 0 OR s.created_at > ?5)
              ORDER BY s.created_at DESC LIMIT ?6
           )
         ) AS links
         FROM users u WHERE u.id = ?3
       ) WHERE banned_at IS NOT NULL OR links != '[]'
       ON CONFLICT (hash) DO UPDATE SET link_times = excluded.link_times, banned_at = excluded.banned_at, left_at = excluded.left_at`,
    )
    .bind(departed, now, me, now - WEEK_MS, now - PENDING_MS, KEPT_LINKS);
}

export async function tokenHash(token: string): Promise<string> {
  return base64url(await sha256(token));
}

const SESSION_IDLE_MS = 180 * 24 * 60 * 60 * 1000;
const TOUCH_EVERY_MS = 24 * 60 * 60 * 1000;

/** The user behind a bearer token, or a 401. */
export async function authenticate(request: Request, db: D1Database): Promise<string> {
  const header = request.headers.get("Authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice(7).trim() : "";
  if (!token) throw new HttpError(401, "Not signed in.");
  const hash = await tokenHash(token);
  const session = await db
    .prepare(
      `SELECT s.user_id, s.last_used, u.banned_at FROM sessions s JOIN users u ON u.id = s.user_id
        WHERE s.token_hash = ?`,
    )
    .bind(hash)
    .first<{ user_id: string; last_used: number; banned_at: number | null }>();
  const now = Date.now();
  if (!session || now - session.last_used > SESSION_IDLE_MS) {
    throw new HttpError(401, "Signed out — please sign in again.");
  }
  // A suspension used to stop only the next sign-in: a session already open
  // kept friends, presence and relays going, and could even delete the
  // account and with it the ban. A 401, not a 403 — on that the app signs
  // out, and signing in again shows the suspension page.
  if (session.banned_at !== null) {
    throw new HttpError(401, "This account is suspended.");
  }
  // One write a day is enough to keep a session alive.
  if (now - session.last_used > TOUCH_EVERY_MS) {
    await db.prepare("UPDATE sessions SET last_used = ? WHERE token_hash = ?").bind(now, hash).run();
  }
  return session.user_id;
}
