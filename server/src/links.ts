// Share links: a clip lives in R2 for five days under clippiboy.com/c/<id>.
//
// The app shrinks the clip first (to under 50 MB), then streams it here. The
// page at /c/<id> is served by this Worker too — the site proxy forwards that
// one path — and the video itself comes straight from api.clippiboy.com, so the
// big file never makes a second hop through the proxy.

import { HttpError, json } from "./util";
import { renderPage } from "./page";
import {
  byteRange,
  canUpload,
  cleanTags,
  expiresIn,
  ID_PATTERN,
  LIFETIME_MS,
  MAX_BYTES,
  MAX_POSTER_BYTES,
  newId,
  quota,
  WEEK_MS,
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
}

const video = (id: string) => `${id}.mp4`;
const poster = (id: string) => `${id}.jpg`;

async function recentOf(env: Env, me: string, now: number): Promise<number[]> {
  const { results } = await env.DB.prepare("SELECT created_at FROM shares WHERE owner = ? AND created_at > ?")
    .bind(me, now - WEEK_MS)
    .all<{ created_at: number }>();
  return results.map((row) => row.created_at);
}

async function liveBytes(env: Env): Promise<number> {
  const row = await env.DB.prepare("SELECT COALESCE(SUM(bytes), 0) AS total FROM shares WHERE deleted_at IS NULL")
    .first<{ total: number }>();
  return row?.total ?? 0;
}

async function live(env: Env, id: string, now: number): Promise<ShareRow | null> {
  if (!ID_PATTERN.test(id)) return null;
  const row = await env.DB.prepare("SELECT * FROM shares WHERE id = ?").bind(id).first<ShareRow>();
  if (!row || row.deleted_at !== null || row.expires_at <= now) return null;
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
  return json(quota(await recentOf(env, me, now), await liveBytes(env)));
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
  const verdict = canUpload(await recentOf(env, me, now), bytes, await liveBytes(env));
  if (!verdict.ok) {
    if (verdict.reason === "too_big") {
      throw new HttpError(413, `Clips for links can be ${MAX_BYTES / 1024 / 1024} MB at most.`);
    }
    if (verdict.reason === "quota") {
      return json({ error: "No links left this week.", reason: "quota", resetsAt: verdict.resetsAt }, 429);
    }
    return json({ error: "Sharing is full right now — try again later.", reason: "full" }, 507);
  }

  const url = new URL(request.url);
  const text = (name: string, max: number) => (url.searchParams.get(name) ?? "").trim().slice(0, max);
  const size = (name: string) => Math.max(0, Math.min(16384, Math.round(Number(url.searchParams.get(name)) || 0)));
  const id = newId();
  const expiresAt = now + LIFETIME_MS;

  // The row first: it is what counts against the week and the bucket, so two
  // uploads at once cannot both slip under the ceiling unseen.
  const tags = JSON.stringify(cleanTags(url.searchParams.getAll("tag")));
  const showName = url.searchParams.get("name") === "1" ? 1 : 0;
  await env.DB.prepare(
    `INSERT INTO shares (id, owner, bytes, title, game, width, height, created_at, expires_at, tags, show_name)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
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
    )
    .run();
  try {
    // A fixed-length stream: R2 needs the size up front, and a body that turns
    // out longer than it said is cut off rather than stored.
    const sized = new FixedLengthStream(bytes);
    const piping = request.body.pipeTo(sized.writable);
    await env.CLIPS.put(video(id), sized.readable, {
      httpMetadata: { contentType: "video/mp4" },
    });
    await piping;
  } catch (err) {
    await env.DB.prepare("DELETE FROM shares WHERE id = ?").bind(id).run();
    console.error("upload", id, err);
    throw new HttpError(502, "The upload broke off. Try again.");
  }
  return json({ id, url: `${PAGE_ORIGIN}/c/${id}`, expiresAt });
}

export async function putPoster(request: Request, env: Env, me: string, id: string): Promise<Response> {
  const row = await owned(env, me, id);
  if (row.deleted_at !== null) throw new HttpError(404, "That link is gone.");
  const data = await request.arrayBuffer();
  if (data.byteLength === 0 || data.byteLength > MAX_POSTER_BYTES) throw new HttpError(413, "Poster too large.");
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
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Clip expired · ClippiBoy</title><meta name="robots" content="noindex">
<style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#08080a;color:#f4f4f5;font:16px/1.5 system-ui,sans-serif;padding:16px}
main{background:#121215;border:1px solid #26262c;border-radius:20px;padding:32px 36px;max-width:380px;text-align:center}
h1{font-size:20px;margin:0 0 8px}p{color:#a1a1aa;margin:0}a{display:inline-block;margin-top:20px;background:#fff;color:#08080a;border-radius:999px;padding:10px 20px;text-decoration:none;font-weight:600}</style></head>
<body><main><h1>This clip has expired</h1><p>Links from ClippiBoy last five days.</p><a href="${PAGE_ORIGIN}">Get ClippiBoy</a></main></body></html>`;
  return new Response(html, { status: 410, headers: { "Content-Type": "text/html; charset=utf-8" } });
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
  // A week to count against, plus a margin.
  await env.DB.prepare("DELETE FROM shares WHERE deleted_at IS NOT NULL AND created_at < ?")
    .bind(now - 2 * WEEK_MS)
    .run();
}
