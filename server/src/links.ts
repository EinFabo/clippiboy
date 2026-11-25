// Share links: a clip lives in R2 for five days under clippiboy.com/c/<id>.
//
// The app shrinks the clip first (to under 50 MB), then streams it here. The
// page at /c/<id> is served by this Worker too — the site proxy forwards that
// one path — and the video itself comes straight from api.clippiboy.com, so the
// big file never makes a second hop through the proxy.

import { HttpError, json } from "./util";
import { addressKey, readLimited } from "./limits";
import { renderGone, renderPage } from "./page";
import { inviteKey } from "./invite";
import { fingerprint } from "./departed";
import { avatarRoute, discordAvatar, looksLikePng, MAX_AVATAR_BYTES } from "./avatar";
import {
  canUpload,
  chunks,
  cleanReport,
  cleanTags,
  MAX_OPEN_REPORTS,
  expiresIn,
  FREE_FAILS,
  friendOffer,
  HELD_MS,
  ID_PATTERN,
  LIFETIME_MS,
  looksLikeJpeg,
  MAX_BYTES,
  MAX_POSTER_BYTES,
  Mp4Boxes,
  newId,
  offersFriend,
  parseRange,
  PENDING_MS,
  posterRefusal,
  quota,
  REPORT_KEEP_MS,
  TOTAL_BYTES,
  WEEK_MS,
  WEEKLY_LIMIT,
  weekStart,
  weekTimes,
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
  /** "Add <name> as friend" offered on this link (Settings → Friends). */
  offer_friend: number;
  pending: number;
  /** An upload that failed (GAVE_UP), kept so it can count against the week. */
  failed: number;
}

const video = (id: string) => `${id}.mp4`;
const poster = (id: string) => `${id}.jpg`;

// What counts against the week and the bucket (keep `departStatement` in db.ts,
// which carries the newest of these over a deleted account, in step with it): finished shares, and uploads
// still in time to finish. The `?` is when a pending one is given up. The same
// rule as `countsAgainstWeek` in shares.ts.
const COUNTS = "(pending = 0 OR created_at > ?)";
// How many count against the week: every link, and failed uploads past the
// free ones (`weekTimes` in shares.ts).
const RECENT = `SELECT COALESCE(SUM(failed = 0), 0) + MAX(0, COALESCE(SUM(failed), 0) - ${FREE_FAILS})
  FROM shares WHERE owner = ? AND created_at > ? AND ${COUNTS}`;
const LIVE_BYTES = `SELECT COALESCE(SUM(bytes), 0) FROM shares WHERE deleted_at IS NULL AND ${COUNTS}`;

// For testing: accounts listed in the secret QUICK_LINKS_FOR (ids, comma
// separated) get links that last five minutes and no weekly limit. Unset, it
// changes nothing. `wrangler secret delete QUICK_LINKS_FOR` ends it.
const QUICK_LIFETIME_MS = 5 * 60 * 1000;

function quick(env: Env, me: string): boolean {
  const list = (env as Env & { QUICK_LINKS_FOR?: string }).QUICK_LINKS_FOR ?? "";
  return list.split(",").some((id) => id.trim() === me);
}

/** What moderation says about an account's links. */
interface Standing {
  banned: boolean;
  /** No weekly limit — the size and bucket limits still hold. */
  unlimited: boolean;
  /** Links made before this no longer count against the week. */
  resetAt: number;
}

async function standingOf(env: Env, me: string): Promise<Standing> {
  const row = await env.DB.prepare("SELECT banned_at, unlimited_links, links_reset_at FROM users WHERE id = ?")
    .bind(me)
    .first<{ banned_at: number | null; unlimited_links: number; links_reset_at: number }>();
  return {
    banned: row?.banned_at != null,
    unlimited: row?.unlimited_links === 1 || quick(env, me),
    resetAt: row?.links_reset_at ?? 0,
  };
}

async function recentOf(env: Env, me: string, now: number, standing: Standing): Promise<number[]> {
  if (standing.unlimited) return [];
  const { results } = await env.DB.prepare(`SELECT created_at, failed FROM shares WHERE owner = ? AND created_at > ? AND ${COUNTS}`)
    .bind(me, weekStart(now, standing.resetAt), now - PENDING_MS)
    .all<{ created_at: number; failed: number }>();
  return weekTimes(results);
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
  // A suspended account's links are down at once, not only after the cron.
  const row = await env.DB.prepare(
    "SELECT s.* FROM shares s JOIN users u ON u.id = s.owner WHERE s.id = ? AND u.banned_at IS NULL",
  )
    .bind(id)
    .first<ShareRow>();
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
const TOMBSTONE = "UPDATE shares SET deleted_at = ?, title = '', game = NULL, tags = '[]', show_name = 0, offer_friend = 0 WHERE id = ?";
// An upload that failed: the same, finished, empty and marked failed. A few
// a week are free, past that each counts like a link (`weekTimes`). Removed
// instead, a failing upload cost nothing and could be tried over and over,
// every try a row written and deleted and two R2 writes. `givenUp` in
// shares.ts is the same in code.
const GAVE_UP =
  "UPDATE shares SET pending = 0, failed = 1, bytes = 0, deleted_at = ?, title = '', game = NULL, tags = '[]', show_name = 0, offer_friend = 0 WHERE id = ? AND pending = 1";

// --- For the app (signed in) --------------------------------------------------

export async function shareQuota(env: Env, me: string): Promise<Response> {
  const now = Date.now();
  const standing = await standingOf(env, me);
  return json(quota(await recentOf(env, me, now, standing), await liveBytesCached(env, now)));
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
  const standing = await standingOf(env, me);
  if (standing.banned) throw new HttpError(403, "This account is suspended.");
  const refuse = async () => {
    const verdict = canUpload(await recentOf(env, me, now, standing), bytes, await liveBytes(env, now));
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
  const offerFriend = friendOffer(url.searchParams.get("friend"));
  const inserted = await env.DB.prepare(
    `INSERT INTO shares (id, owner, bytes, title, game, width, height, created_at, expires_at, tags, show_name, offer_friend, pending)
     SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1
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
      offerFriend,
      me,
      weekStart(now, standing.resetAt),
      now - PENDING_MS,
      standing.unlimited ? Number.MAX_SAFE_INTEGER : WEEKLY_LIMIT,
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
    await env.CLIPS.delete([video(id), poster(id)]).catch(() => {});
    await env.DB.prepare(GAVE_UP).bind(Date.now(), id).run();
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
    await env.CLIPS.delete([video(id), poster(id)]).catch(() => {});
    // Still there and pending (too slow, not deleted): it counts like any failed upload.
    await env.DB.prepare(GAVE_UP).bind(Date.now(), id).run();
    throw new HttpError(410, "The upload took too long. Try again.");
  }
  return json({ id, url: `${PAGE_ORIGIN}/c/${id}`, expiresAt });
}

export async function putPoster(request: Request, env: Env, me: string, id: string): Promise<Response> {
  const row = await owned(env, me, id);
  // Only once the clip is up: a poster put while it was still uploading stayed
  // in R2 when the upload failed.
  const refusal = posterRefusal(row);
  if (refusal === "gone") throw new HttpError(404, "That link is gone.");
  if (refusal === "uploading") throw new HttpError(409, "The clip is still uploading.");
  // The length before the body: reading first would hold whatever was sent
  // in memory, and a big enough one takes the Worker down for everyone.
  const length = Number(request.headers.get("Content-Length"));
  if (!request.body || !Number.isFinite(length) || length <= 0 || length > MAX_POSTER_BYTES) {
    throw new HttpError(413, "Poster too large.");
  }
  // One per link: the app sends it once, right after the upload. Replaced over
  // and over, it was an R2 write each time for nothing — Discord keeps the
  // first one it saw anyway.
  if (await env.CLIPS.head(poster(id))) throw new HttpError(409, "This link has its picture already.");
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
    await dropClips(env, [id], Date.now());
    await env.DB.prepare(TOMBSTONE).bind(Date.now(), id).run();
  }
  return json({ ok: true });
}

/**
 * Takes these links' clips and posters out of R2 — all but those with an open
 * report: the review needs them, so they are held (`held_clips`) until no
 * report is open, `HELD_MS` at most (the sweep's "held" step). Nobody reaches
 * them meanwhile: the caller takes the link down with its row. Deleting a
 * reported clip used to take the evidence along, and the sweep did so five
 * days after any upload.
 */
async function dropClips(env: Env, ids: string[], now: number): Promise<void> {
  if (ids.length === 0) return;
  const { results } = await env.DB.prepare(
    "SELECT DISTINCT share_id FROM reports WHERE status = 'open' AND share_id IN (SELECT value FROM json_each(?))",
  )
    .bind(JSON.stringify(ids))
    .all<{ share_id: string }>();
  const reported = new Set(results.map((row) => row.share_id));
  if (reported.size > 0) {
    await env.DB.batch(
      [...reported].map((id) =>
        env.DB.prepare("INSERT OR IGNORE INTO held_clips (share_id, held_at) VALUES (?, ?)").bind(id, now),
      ),
    );
  }
  const gone = ids.filter((id) => !reported.has(id));
  // R2 deletes at most a thousand keys at once; two per clip.
  for (const run of chunks(gone, SUSPEND_BATCH)) {
    await env.CLIPS.delete(run.flatMap((id) => [video(id), poster(id)]));
  }
}

/**
 * Before an account goes: its clips go with it, not five days later. In runs
 * R2 accepts (two keys a link): an account without the weekly limit can have
 * more links than one call may delete, and then deleting the account failed.
 */
export async function deleteSharesOf(env: Env, me: string): Promise<void> {
  const { results } = await env.DB.prepare("SELECT id FROM shares WHERE owner = ? AND deleted_at IS NULL")
    .bind(me)
    .all<{ id: string }>();
  await dropClips(
    env,
    results.map((row) => row.id),
    Date.now(),
  );
}

// --- For everyone ---------------------------------------------------------------

/**
 * `POST /c/<id>/report` from the share page, `{reason, note?}` sent as plain
 * text so the browser needs no preflight. Who reported is not kept, nor the
 * address: only a keyed one-way fingerprint of it (its /48 for IPv6) made
 * for this clip alone, so each address counts once against the clip's cap
 * (`takesReport`) and none can be told apart across clips. It goes with the
 * report.
 */
export async function reportShare(request: Request, env: Env, id: string): Promise<Response> {
  const cors = { "Access-Control-Allow-Origin": PAGE_ORIGIN };
  const answer = (body: unknown, status = 200) => Response.json(body, { status, headers: cors });
  const address = request.headers.get("CF-Connecting-IP") ?? "unknown";
  if (!(await env.REPORT_LIMIT.limit({ key: addressKey(address) })).success) {
    return answer({ error: "Too many reports — try again in a minute." }, 429);
  }
  let raw: unknown = null;
  try {
    // Over 2 KB it is not read at all; null is refused below like bad JSON.
    raw = JSON.parse((await readLimited(request, 2048)) ?? "");
  } catch {
    // Not JSON: refused below.
  }
  const report = cleanReport(raw);
  if (!report) return answer({ error: "Pick a reason." }, 400);
  if (!(await live(env, id, Date.now()))) return answer({ error: "That clip is gone." }, 404);
  const reporter = await fingerprint(env.DEPARTED_PEPPER || env.DISCORD_CLIENT_SECRET, `report\n${id}\n${addressKey(address)}`);
  // `takesReport` in shares.ts; without a key to make the fingerprint with,
  // the cap counts reports as it did before.
  const takes = reporter
    ? `NOT EXISTS (SELECT 1 FROM reports WHERE share_id = ?2 AND status = 'open' AND reporter = ?6)
       AND (SELECT COUNT(DISTINCT reporter) FROM reports WHERE share_id = ?2 AND status = 'open') < ?7`
    : "(SELECT COUNT(*) FROM reports WHERE share_id = ?2 AND status = 'open') < ?7";
  await env.DB.prepare(
    `INSERT INTO reports (id, share_id, reason, note, created_at, reporter)
     SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE ${takes}`,
  )
    .bind(newId(), id, report.reason, report.note, Date.now(), reporter, MAX_OPEN_REPORTS)
    .run();
  // Over the cap it is quietly dropped: the clip is waiting for review anyway.
  return answer({ ok: true });
}

export interface Uploader {
  id: string;
  display_name: string;
  avatar: string | null;
  friend_code: string;
  allow_requests: number;
}

/** Who shared a live link — only when they let the page show their name. */
async function uploaderOf(env: Env, row: ShareRow): Promise<Uploader | null> {
  if (!row.show_name) return null;
  return env.DB.prepare("SELECT id, display_name, avatar, friend_code, allow_requests FROM users WHERE id = ?")
    .bind(row.owner)
    .first<Uploader>();
}

/** The uploader of a live link, for "Add <name> as friend" — null for a gone link, a hidden name or no button offered. */
export async function shareUploader(env: Env, id: string): Promise<Uploader | null> {
  const row = await live(env, id, Date.now());
  if (!row) return null;
  const uploader = await uploaderOf(env, row);
  return uploader && offersFriend(row, uploader.allow_requests) ? uploader : null;
}

export async function sharePage(env: Env, id: string): Promise<Response> {
  const now = Date.now();
  const row = await live(env, id, now);
  if (!row) return gone();
  const uploader = await uploaderOf(env, row);
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
    report: `${MEDIA_ORIGIN}/c/${id}/report`,
    title: row.title,
    game: row.game,
    tags,
    // Through our route: Discord's link carries the Discord id.
    uploader: uploader
      ? { name: uploader.display_name, avatar: discordAvatar(uploader.avatar) ? avatarRoute(MEDIA_ORIGIN, id) : null }
      : null,
    addFriend:
      uploader && offersFriend(row, uploader.allow_requests) && inviteKey(env.INVITE_SECRET, env.DISCORD_CLIENT_SECRET)
        ? `${PAGE_ORIGIN}/c/${id}/add`
        : null,
    expires: expiresIn(row.expires_at, now),
    width: row.width,
    height: row.height,
    site: PAGE_ORIGIN,
  });
  return new Response(html, {
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
      // Its "Add as friend" must not be clicked through someone else's frame.
      "Content-Security-Policy": "frame-ancestors 'none'",
    },
  });
}

/**
 * The video or the poster, with seeking (Range) for the video. HEAD reads only
 * the object's details, and a copy the browser still has (If-None-Match) is
 * answered with a 304 and no body.
 */
export async function shareFile(request: Request, env: Env, id: string, kind: "mp4" | "jpg"): Promise<Response> {
  const now = Date.now();
  const row = await live(env, id, now);
  if (!row) return new Response("This clip has expired.", { status: 410 });
  const key = kind === "mp4" ? video(id) : poster(id);
  // The video's size is the row's: the upload was stored at exactly that
  // length. A poster is small and always sent whole.
  const range = kind === "mp4" ? parseRange(request.headers.get("Range"), row.bytes) : ({ kind: "whole" } as const);
  if (range.kind === "unsatisfiable") {
    return new Response(null, { status: 416, headers: { "Content-Range": `bytes */${row.bytes}` } });
  }
  const head = request.method === "HEAD";
  const object = head
    ? await env.CLIPS.head(key)
    : await env.CLIPS.get(key, {
        range: range.kind === "part" ? { offset: range.offset, length: range.length } : undefined,
        onlyIf: ifNoneMatch(request.headers),
      });
  if (!object) return new Response("Not found.", { status: 404 });

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("ETag", object.httpEtag);
  headers.set("Accept-Ranges", "bytes");
  // Never sniffed into something else: only ever a video or a picture.
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Content-Disposition", "inline");
  headers.set("Access-Control-Allow-Origin", "*");
  // A link is not for search engines, and once deleted it should not live on
  // in shared caches: only the viewer's browser keeps it, and only briefly.
  headers.set("X-Robots-Tag", "noindex");
  const left = Math.max(0, Math.floor((row.expires_at - now) / 1000));
  headers.set("Cache-Control", `private, max-age=${Math.min(left, 300)}`);
  // The condition failed: R2 sent the details without the body.
  if (!head && !("body" in object)) return new Response(null, { status: 304, headers });

  if (range.kind === "part") {
    headers.set("Content-Range", `bytes ${range.offset}-${range.offset + range.length - 1}/${object.size}`);
    headers.set("Content-Length", String(range.length));
  } else {
    headers.set("Content-Length", String(object.size));
  }
  const body = head ? null : (object as R2ObjectBody).body;
  return new Response(body, { status: range.kind === "part" ? 206 : 200, headers });
}

/**
 * `/c/<id>/avatar`: the uploader's picture for the share page, fetched from
 * Discord here so the page never carries the Discord id that Discord's link
 * has. Only while the link is up and shows the name, only a PNG from
 * Discord's avatar paths (`discordAvatar`), and only small.
 */
export async function shareAvatar(request: Request, env: Env, id: string): Promise<Response> {
  const row = await live(env, id, Date.now());
  const source = row ? discordAvatar((await uploaderOf(env, row))?.avatar ?? null) : null;
  if (!source) return new Response("Not found.", { status: 404 });
  let bytes: Uint8Array | null = null;
  try {
    const upstream = await fetch(source, { cf: { cacheEverything: true, cacheTtl: 86_400 }, redirect: "manual" });
    const type = upstream.headers.get("Content-Type") ?? "";
    if (upstream.ok && type.split(";")[0].trim() === "image/png") {
      const text = await readBytes(upstream, MAX_AVATAR_BYTES);
      if (text && looksLikePng(text)) bytes = text;
    } else {
      await upstream.body?.cancel();
    }
  } catch (err) {
    console.error("avatar", id, err);
  }
  if (!bytes) return new Response("Not found.", { status: 404 });
  return new Response(request.method === "HEAD" ? null : bytes, {
    headers: {
      "Content-Type": "image/png",
      "Content-Length": String(bytes.byteLength),
      "X-Content-Type-Options": "nosniff",
      "Content-Security-Policy": "default-src 'none'",
      "X-Robots-Tag": "noindex",
      // Gone with the link or the name: only the viewer's browser keeps it, briefly.
      "Cache-Control": "private, max-age=600",
    },
  });
}

/** The body, or null once it is past `max` bytes. */
async function readBytes(response: Response, max: number): Promise<Uint8Array | null> {
  const declared = Number(response.headers.get("Content-Length") ?? "");
  if (!response.body || (Number.isFinite(declared) && declared > max)) {
    await response.body?.cancel();
    return null;
  }
  const reader = response.body.getReader();
  const parts: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel();
      return null;
    }
    parts.push(value);
  }
  const out = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
}

/** Only the request's If-None-Match, for R2 to check — not its other headers. */
function ifNoneMatch(request: Headers): Headers | undefined {
  const etag = request.get("If-None-Match");
  return etag ? new Headers({ "If-None-Match": etag }) : undefined;
}

/**
 * The page of a link that is gone. 200, not 410: Discord builds a preview only
 * from a 2xx answer (docs.discord.com/developers/link-previews), and the
 * preview is the invitation. The page says `noindex` itself (`head` in
 * page.ts), and `no-store` keeps any cache from holding it.
 */
function gone(): Response {
  return new Response(renderGone(PAGE_ORIGIN), {
    status: 200,
    headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" },
  });
}

// --- Hourly -----------------------------------------------------------------------

/** Links taken down per step: R2 deletes at most 1000 keys at once, two per link. */
const SUSPEND_BATCH = 400;

/**
 * Carry suspensions through: sign the account out everywhere, take its links
 * down and drop its open connections, within the hour of `banned_at` being
 * set. Each suspension once: `suspension_applied_at` keeps the `banned_at` it
 * was carried out for, so a suspension set anew is carried out anew.
 */
export async function applySuspensions(env: Env): Promise<void> {
  const { results: banned } = await env.DB.prepare(
    `SELECT id, banned_at FROM users WHERE banned_at IS NOT NULL
       AND (suspension_applied_at IS NULL OR suspension_applied_at < banned_at)`,
  ).all<{ id: string; banned_at: number }>();
  for (const { id, banned_at } of banned) {
    // One that fails must not hold up everyone after it, hour after hour.
    try {
      await suspend(env, id, banned_at);
    } catch (err) {
      console.error("suspension", id, err);
    }
  }
}

async function suspend(env: Env, id: string, bannedAt: number): Promise<void> {
  for (;;) {
    const { results: shares } = await env.DB.prepare(
      "SELECT id FROM shares WHERE owner = ? AND deleted_at IS NULL LIMIT ?",
    )
      .bind(id, SUSPEND_BATCH)
      .all<{ id: string }>();
    if (shares.length === 0) break;
    const now = Date.now();
    await dropClips(
      env,
      shares.map((row) => row.id),
      now,
    );
    await env.DB.batch(shares.map((row) => env.DB.prepare(TOMBSTONE).bind(now, row.id)));
  }
  await env.DB.prepare("DELETE FROM sessions WHERE user_id = ?").bind(id).run();
  await env.HUB.getByName(id).closeAll("account suspended", id);
  // Last: whatever failed above is tried again next hour.
  await env.DB.prepare("UPDATE users SET suspension_applied_at = ? WHERE id = ?").bind(bannedAt, id).run();
}

/**
 * Delete what has run out, and forget rows that no longer count for anything.
 * Each step on its own: one that fails (R2 or D1 having a bad moment) must not
 * keep the others from running this hour.
 */
export async function sweep(env: Env): Promise<void> {
  const now = Date.now();
  const step = async (name: string, run: () => Promise<unknown>) => {
    try {
      await run();
    } catch (err) {
      console.error("sweep", name, err);
    }
  };
  await step("expired", async () => {
    const { results } = await env.DB.prepare(
      "SELECT id FROM shares WHERE deleted_at IS NULL AND expires_at <= ? LIMIT 400",
    )
      .bind(now)
      .all<{ id: string }>();
    if (results.length > 0) {
      await dropClips(
        env,
        results.map((row) => row.id),
        now,
      );
      await env.DB.batch(
        results.map((row) => env.DB.prepare(TOMBSTONE).bind(now, row.id)),
      );
    }
  });
  // Uploads that broke off without their catch running: whatever reached R2
  // goes, and the row is given up like any failed upload (`givenUp`). Deleted
  // instead, an upload cut off on purpose cost nothing against the week.
  await step("stale", async () => {
    const { results: stale } = await env.DB.prepare("SELECT id FROM shares WHERE pending = 1 AND created_at <= ? LIMIT 400")
      .bind(now - PENDING_MS)
      .all<{ id: string }>();
    if (stale.length > 0) {
      await env.CLIPS.delete(stale.flatMap((row) => [video(row.id), poster(row.id)]));
      await env.DB.batch(stale.map((row) => env.DB.prepare(GAVE_UP).bind(now, row.id)));
    }
  });
  // A week to count against, plus a margin.
  await step("tombstones", () =>
    env.DB.prepare("DELETE FROM shares WHERE deleted_at IS NOT NULL AND created_at < ?")
      .bind(now - 2 * WEEK_MS)
      .run(),
  );
  // Reports are for reviewing, not for keeping.
  await step("reports", () => env.DB.prepare("DELETE FROM reports WHERE created_at < ?").bind(now - REPORT_KEEP_MS).run());
  // Clips held for a report (`dropClips`): gone once no report on them is
  // open, and after HELD_MS in any case (`heldClipGoes`).
  await step("held", async () => {
    const { results: done } = await env.DB.prepare(
      `SELECT share_id FROM held_clips h
        WHERE h.held_at <= ? OR NOT EXISTS (SELECT 1 FROM reports r WHERE r.share_id = h.share_id AND r.status = 'open')
        LIMIT 400`,
    )
      .bind(now - HELD_MS)
      .all<{ share_id: string }>();
    if (done.length > 0) {
      await env.CLIPS.delete(done.flatMap((row) => [video(row.share_id), poster(row.share_id)]));
      await env.DB.prepare("DELETE FROM held_clips WHERE share_id IN (SELECT value FROM json_each(?))")
        .bind(JSON.stringify(done.map((row) => row.share_id)))
        .run();
    }
  });
}
