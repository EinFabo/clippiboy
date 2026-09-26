import { friendCode, HttpError, sha256, base64url } from "./util";

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
  return db.prepare("SELECT * FROM users WHERE username = ?").bind(name).first<UserRow>();
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

/** Creates the account on the first login and keeps name and avatar in step after. */
export async function upsertDiscordUser(db: D1Database, discord: DiscordUser): Promise<string> {
  const displayName = discord.global_name || discord.username;
  const avatar = avatarUrl(discord);
  const existing = await db
    .prepare("SELECT id FROM users WHERE discord_id = ?")
    .bind(discord.id)
    .first<{ id: string }>();
  if (existing) {
    await db
      .prepare("UPDATE users SET username = ?, display_name = ?, avatar = ? WHERE id = ?")
      .bind(discord.username, displayName, avatar, existing.id)
      .run();
    return existing.id;
  }
  const id = crypto.randomUUID();
  // A collision among 31^8 codes is unlikely, not impossible.
  for (let attempt = 0; attempt < 5; attempt++) {
    const result = await db
      .prepare(
        "INSERT INTO users (id, discord_id, username, display_name, avatar, friend_code, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (friend_code) DO NOTHING",
      )
      .bind(id, discord.id, discord.username, displayName, avatar, friendCode(), Date.now())
      .run();
    if (result.meta.changes === 1) return id;
  }
  throw new HttpError(500, "Could not create the account.");
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
    .prepare("SELECT user_id, last_used FROM sessions WHERE token_hash = ?")
    .bind(hash)
    .first<{ user_id: string; last_used: number }>();
  const now = Date.now();
  if (!session || now - session.last_used > SESSION_IDLE_MS) {
    throw new HttpError(401, "Signed out — please sign in again.");
  }
  // One write a day is enough to keep a session alive.
  if (now - session.last_used > TOUCH_EVERY_MS) {
    await db.prepare("UPDATE sessions SET last_used = ? WHERE token_hash = ?").bind(now, hash).run();
  }
  return session.user_id;
}
