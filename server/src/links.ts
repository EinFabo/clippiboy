// Share links: a clip lives in R2 for five days under clippiboy.com/c/<id>.
//
// The app shrinks the clip first (to under 50 MB), then streams it here. The
// page at /c/<id> is served by this Worker too — the site proxy forwards that
// one path — and the video itself comes straight from api.clippiboy.com, so the
// big file never makes a second hop through the proxy.

import { HttpError, json } from "./util";
import { renderGone, renderPage } from "./page";
import {
  byteRange,
  canUpload,
  cleanTags,
  expiresIn,
  ID_PATTERN,
  LIFETIME_MS,
  looksLikeJpeg,
  MAX_BYTES,
  MAX_POSTER_BYTES,
  Mp4Boxes,
  newId,
  PENDING_MS,
  quota,
  TOTAL_BYTES,
  WEEK_MS,
  WEEKLY_LIMIT,
} from "./shares";

export const PAGE_ORIGIN = "https://clippiboy.com";
export const MEDIA_ORIGIN = "https://api.clippiboy.com";

interface ShareRow {
  id: string;
  owner: string;
  bytes: number;
  title: string;
  game: string | null;
  width: number;
  height: number;
  created_at: number;
  expires_at: number;
  deleted_at: number | null;
  tags: string;
  show_name: number;
  pending: number;
}

const video = (id: string) => `${id}.mp4`;
const poster = (id: string) => `${id}.jpg`;

// What counts against the week and the bucket: finished shares, and uploads
// still in time to finish. The `?` is when a pending one is given up.
const COUNTS = "(pending = 0 OR created_at > ?)";
const RECENT = `SELECT COUNT(*) FROM shares WHERE owner = ? AND created_at > ? AND ${COUNTS}`;
const LIVE_BYTES = `SELECT COALESCE(SUM(bytes), 0) FROM shares WHERE deleted_at IS NULL AND ${COUNTS}`;

// For testing: accounts listed in the secret QUICK_LINKS_FOR (ids, comma
// separated) get links that last five minutes and no weekly limit. Unset, it
// changes nothing. `wrangler secret delete QUICK_LINKS_FOR` ends it.
const QUICK_LIFETIME_MS = 5 * 60 * 1000;

function quick(env: Env, me: string): boolean {
  const list = (env as Env & { QUICK_LINKS_FOR?: string }).QUICK_LINKS_FOR ?? "";
  return list.split(",").some((id) => id.trim() === me);
}

async function recentOf(env: Env, me: string, now: number): Promise<number[]> {
  if (quick(env, me)) return [];
  const { results } = await env.DB.prepare(`SELECT created_at FROM shares WHERE owner = ? AND created_at > ? AND ${COUNTS}`)
    .bind(me, now - WEEK_MS, now - PENDING_MS)
    .all<{ created_at: number }>();
  return results.map((row) => row.created_at);
}

async function liveBytes(env: Env, now: number): Promise<number> {
  const row = await env.DB.prepare(`SELECT COALESCE(SUM(bytes), 0) AS total FROM shares WHERE deleted_at IS NULL AND ${COUNTS}`)
    .bind(now - PENDING_MS)
    .first<{ total: number }>();
  return row?.total ?? 0;
}

// The menu in every running app asks for the quota, and summing the bucket
// reads every live row. For that answer a few minutes old is close enough —
// an upload sums afresh, inside its INSERT.
const BUCKET_TTL_MS = 5 * 60 * 1000;
let bucket: { total: number; at: number } | null = null;

async function liveBytesCached(env: Env, now: number): Promise<number> {
  if (bucket && now - bucket.at < BUCKET_TTL_MS) return bucket.total;
  const total = await liveBytes(env, now);
  bucket = { total, at: now };
  return total;
}

/** Passes the upload on, but fails it the moment it stops being an MP4. */
function onlyMp4(): { stream: TransformStream<Uint8Array, Uint8Array>; refused: () => boolean } {
  const boxes = new Mp4Boxes();
  let refused = false;
  const refuse = () => {
    refused = true;
    throw new Error("not an mp4");
  };
  const stream = new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      if (!boxes.push(chunk)) refuse();
      controller.enqueue(chunk);
    },
    flush() {
      if (!boxes.end()) refuse();
    },
  });
  return { stream, refused: () => refused };
}

async function live(env: Env, id: string, now: number): Promise<ShareRow | null> {
  if (!ID_PATTERN.test(id)) return null;
  const row = await env.DB.prepare("SELECT * FROM shares WHERE id = ?").bind(id).first<ShareRow>();
  if (!row || row.pending || row.deleted_at !== null || row.expires_at <= now) return null;
  return row;
}

async function owned(env: Env, me: string, id: string): Promise<ShareRow> {
  const row = ID_PATTERN.test(id)
    ? await env.DB.prepare("SELECT * FROM shares WHERE id = ?").bind(id).first<ShareRow>()
    : null;
  if (!row) throw new HttpError(404, "That link is gone.");
  // Not a 404: the app would take that as "already deleted" and forget a link
  // that is still up.
  if (row.owner !== me) throw new HttpError(403, "That link belongs to another account.");
  return row;
}

// A deleted or run-out share keeps only what counts against the week and the
// bucket — not what the clip was called, its game or its tags.
const TOMBSTONE = "UPDATE shares SET deleted_at = ?, title = '', game = NULL, tags = '[]', show_name = 0 WHERE id = ?";

// --- For the app (signed in) --------------------------------------------------

export async function shareQuota(env: Env, me: string): Promise<Response> {
  const now = Date.now();
  return json(quota(await recentOf(env, me, now), await liveBytesCached(env, now)));
}

/**
 * The clip as the request body, its description in the query:
 * `POST /shares?title=…&game=…&width=1920&height=1080&tag=…&tag=…&name=1`.
 * `name=1` puts the uploader's Discord name on the page — taken from the
 * account, never from the request.
 */
export async function createShare(request: Request, env: Env, me: string): Promise<Response> {
  const now = Date.now();
  const bytes = Number(request.headers.get("Content-Length"));
  if (!Number.isFinite(bytes) || bytes <= 0 || !request.body) {
    throw new HttpError(411, "The clip needs a length.");
  }
  const refuse = async () => {
    const verdict = canUpload(await recentOf(env, me, now), bytes, await liveBytes(env, now));
    if (verdict.ok) return null;
    if (verdict.reason === "too_big") {
      throw new HttpError(413, `Clips for links can be ${MAX_BYTES / 1024 / 1024} MB at most.`);
    }
    if (verdict.reason === "quota") {
      return json({ error: "No links left this week.", reason: "quota", resetsAt: verdict.resetsAt }, 429);
    }
    return json({ error: "Sharing is full right now — try again later.", reason: "full" }, 507);
  };
  const refused = await refuse();
  if (refused) return refused;

  const url = new URL(request.url);
  const text = (name: string, max: number) => (url.searchParams.get(name) ?? "").trim().slice(0, max);
  const size = (name: string) => Math.max(0, Math.min(16384, Math.round(Number(url.searchParams.get(name)) || 0)));
  const id = newId();
  const expiresAt = now + (quick(env, me) ? QUICK_LIFETIME_MS : LIFETIME_MS);

  // The row first, pending: it is what counts against the week and the
  // bucket. Inserted only if the limits still hold at that moment — the check
  // above alone would let two uploads at once both slip through.
  const tags = JSON.stringify(cleanTags(url.searchParams.getAll("tag")));
  const showName = url.searchParams.get("name") === "1" ? 1 : 0;
  const inserted = await env.DB.prepare(
    `INSERT INTO shares (id, owner, bytes, title, game, width, height, created_at, expires_at, tags, show_name, pending)
     SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1
     WHERE (${RECENT}) < ? AND (${LIVE_BYTES}) + ? <= ?`,
  )
    .bind(
      id,
      me,
      bytes,
      text("title", 120) || "Clip",
      text("game", 80) || null,
      size("width"),
      size("height"),
      now,
      expiresAt,
      tags,
      showName,
      me,
      now - WEEK_MS,
      now - PENDING_MS,
      quick(env, me) ? Number.MAX_SAFE_INTEGER : WEEKLY_LIMIT,
      now - PENDING_MS,
      bytes,
      TOTAL_BYTES,
    )
    .run();
  if (!inserted.meta.changes) {
    return (await refuse()) ?? json({ error: "Sharing is full right now — try again later.", reason: "full" }, 507);
  }
  // Only clips: without this check the link would host any file under our
  // domain for five days.
  const mp4 = onlyMp4();
  try {
    // A fixed-length stream: R2 needs the size up front, and a body that turns
    // out longer than it said is cut off rather than stored.
    const sized = new FixedLengthStream(bytes);
    // Together: when the check refuses, both fail, and an unwatched one would
    // end up in the log as an unhandled rejection.
    await Promise.all([
      env.CLIPS.put(video(id), sized.readable, { httpMetadata: { contentType: "video/mp4" } }),
      request.body.pipeThrough(mp4.stream).pipeTo(sized.writable),
    ]);
  } catch (err) {
    await env.CLIPS.delete(video(id)).catch(() => {});
    await env.DB.prepare("DELETE FROM shares WHERE id = ?").bind(id).run();
    if (mp4.refused()) throw new HttpError(415, "Only clips can be shared as links.");
    console.error("upload", id, err);
    throw new HttpError(502, "The upload broke off. Try again.");
  }
  // The row may have gone while the clip was on its way — the account was
  // deleted — or the upload outlasted PENDING_MS. From then on it no longer
  // counted against the week or the bucket and the sweep may be taking it
  // down: it must not turn into a link, however close the sweep is.
  const finished = await env.DB.prepare(
    "UPDATE shares SET pending = 0 WHERE id = ? AND pending = 1 AND deleted_at IS NULL AND created_at > ?",
  )
    .bind(id, Date.now() - PENDING_MS)
    .run();
  if (!finished.meta.changes) {
    await env.CLIPS.delete(video(id)).catch(() => {});
    throw new HttpError(410, "The upload took too long. Try again.");
  }
  return json({ id, url: `${PAGE_ORIGIN}/c/${id}`, expiresAt });
}

export async function putPoster(request: Request, env: Env, me: string, id: string): Promise<Response> {
  const row = await owned(env, me, id);
  if (row.deleted_at !== null) throw new HttpError(404, "That link is gone.");
  // The length before the body: reading first would hold whatever was sent
  // in memory, and a big enough one takes the Worker down for everyone.
  const length = Number(request.headers.get("Content-Length"));
  if (!request.body || !Number.isFinite(length) || length <= 0 || length > MAX_POSTER_BYTES) {
    throw new HttpError(413, "Poster too large.");
  }
  const data = await request.arrayBuffer();
  if (data.byteLength === 0 || data.byteLength > MAX_POSTER_BYTES) throw new HttpError(413, "Poster too large.");
  if (!looksLikeJpeg(new Uint8Array(data, 0, Math.min(3, data.byteLength)))) {
    throw new HttpError(415, "The poster has to be a JPEG.");
  }
  await env.CLIPS.put(poster(id), data, { httpMetadata: { contentType: "image/jpeg" } });
  return json({ ok: true });
}

export async function deleteShare(env: Env, me: string, id: string): Promise<Response> {
  const row = await owned(env, me, id);
  if (row.deleted_at === null) {
    await env.CLIPS.delete([video(id), poster(id)]);
    await env.DB.prepare(TOMBSTONE).bind(Date.now(), id).run();
  }
  return json({ ok: true });
}

/** Before an account goes: its clips go with it, not five days later. */
export async function deleteSharesOf(env: Env, me: string): Promise<void> {
  const { results } = await env.DB.prepare("SELECT id FROM shares WHERE owner = ? AND deleted_at IS NULL")
    .bind(me)
    .all<{ id: string }>();
  if (results.length > 0) await env.CLIPS.delete(results.flatMap((row) => [video(row.id), poster(row.id)]));
}

// --- For everyone ---------------------------------------------------------------

export async function sharePage(env: Env, id: string): Promise<Response> {
  const now = Date.now();
  const row = await live(env, id, now);
  if (!row) return gone();
  const uploader = row.show_name
    ? await env.DB.prepare("SELECT display_name, avatar FROM users WHERE id = ?")
        .bind(row.owner)
        .first<{ display_name: string; avatar: string | null }>()
    : null;
  let tags: string[] = [];
  try {
    tags = JSON.parse(row.tags);
  } catch {
    // An unreadable list shows no tags rather than no page.
  }
  const html = renderPage({
    id,
    page: `${PAGE_ORIGIN}/c/${id}`,
    media: `${MEDIA_ORIGIN}/c/${video(id)}`,
    poster: `${MEDIA_ORIGIN}/c/${poster(id)}`,
    title: row.title,
    game: row.game,
    tags,
    uploader: uploader ? { name: uploader.display_name, avatar: uploader.avatar } : null,
    expires: expiresIn(row.expires_at, now),
    width: row.width,
    height: row.height,
    site: PAGE_ORIGIN,
  });
  return new Response(html, {
    headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" },
  });
}

/** The video or the poster, with seeking (Range) for the video. */
export async function shareFile(request: Request, env: Env, id: string, kind: "mp4" | "jpg"): Promise<Response> {
  const now = Date.now();
  const row = await live(env, id, now);
  if (!row) return new Response("This clip has expired.", { status: 410 });
  const key = kind === "mp4" ? video(id) : poster(id);
  const object = await env.CLIPS.get(key, { range: request.headers });
  if (!object) return new Response("Not found.", { status: 404 });

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("ETag", object.httpEtag);
  headers.set("Accept-Ranges", "bytes");
  // Never sniffed into something else: only ever a video or a picture.
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Content-Disposition", "inline");
  headers.set("Access-Control-Allow-Origin", "*");
  const left = Math.max(0, Math.floor((row.expires_at - now) / 1000));
  headers.set("Cache-Control", `public, max-age=${Math.min(left, 3600)}`);

  const ranged = request.headers.has("Range") && object.range !== undefined;
  if (ranged) {
    const { offset, length } = byteRange(object.range as { offset?: number; length?: number; suffix?: number }, object.size);
    headers.set("Content-Range", `bytes ${offset}-${offset + length - 1}/${object.size}`);
    headers.set("Content-Length", String(length));
  } else {
    headers.set("Content-Length", String(object.size));
  }
  const body = request.method === "HEAD" || !("body" in object) ? null : (object as R2ObjectBody).body;
  return new Response(body, { status: ranged ? 206 : 200, headers });
}

function gone(): Response {
  return new Response(renderGone(PAGE_ORIGIN), {
    status: 410,
    headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" },
  });
}

// --- Hourly -----------------------------------------------------------------------

/** Delete what has run out, and forget rows that no longer count for anything. */
export async function sweep(env: Env): Promise<void> {
  const now = Date.now();
  const { results } = await env.DB.prepare(
    "SELECT id FROM shares WHERE deleted_at IS NULL AND expires_at <= ? LIMIT 400",
  )
    .bind(now)
    .all<{ id: string }>();
  if (results.length > 0) {
    // R2 takes up to a thousand keys at once; two per clip.
    await env.CLIPS.delete(results.flatMap((row) => [video(row.id), poster(row.id)]));
    await env.DB.batch(
      results.map((row) => env.DB.prepare(TOMBSTONE).bind(now, row.id)),
    );
  }
  // Uploads that broke off without their catch running: whatever reached R2
  // goes, and the row with it — it never was a link.
  const { results: stale } = await env.DB.prepare("SELECT id FROM shares WHERE pending = 1 AND created_at <= ? LIMIT 400")
    .bind(now - PENDING_MS)
    .all<{ id: string }>();
  if (stale.length > 0) {
    await env.CLIPS.delete(stale.flatMap((row) => [video(row.id), poster(row.id)]));
    await env.DB.batch(stale.map((row) => env.DB.prepare("DELETE FROM shares WHERE id = ?").bind(row.id)));
  }
  // A week to count against, plus a margin.
  await env.DB.prepare("DELETE FROM shares WHERE deleted_at IS NOT NULL AND created_at < ?")
    .bind(now - 2 * WEEK_MS)
    .run();
}
