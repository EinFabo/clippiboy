import { addressKey } from "../src/limits";
import { downloadName, isBot, refFromPath, utcDay } from "../src/telemetry";
import { downloadCeiling, HOME, ORIGIN, rewriteLocation, validRelease, withSecurityHeaders } from "./rules";

const RELEASES = "https://github.com/EinFabo/clippiboy/releases/latest";

interface Env {
  API: Fetcher;
  DB: D1Database;
  DOWNLOAD_LIMIT: RateLimit;
  /** The refs campaign links use, comma-separated; any other counts as the site. */
  DOWNLOAD_REFS: string;
  /** Downloads counted a day, all links together (`downloadCeiling`). */
  DOWNLOADS_PER_DAY?: string;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    // One address: www goes to the bare domain.
    if (url.hostname !== "clippiboy.com") {
      return Response.redirect(`${HOME}${url.pathname}${url.search}`, 301);
    }
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed", { status: 405, headers: { Allow: "GET, HEAD" } });
    }
    // A share link's page (the video on it loads from api.clippiboy.com
    // directly), someone's invite link, and "Add <name> as friend" on either.
    // The click's cookie is for all of clippiboy.com, so sign-in on the API
    // reads it.
    if (/^\/c\/[a-z0-9]+(\/add)?$/.test(url.pathname) || /^\/i\/[A-Za-z0-9-]{8,9}(\/add)?$/.test(url.pathname)) {
      return env.API.fetch(request);
    }
    if (url.pathname === "/stats.json") {
      return stats(request, env, ctx);
    }
    if (/^\/get(\/|$)/.test(url.pathname)) {
      const known = (env.DOWNLOAD_REFS ?? "site").split(",").map((ref) => ref.trim().toLowerCase());
      return download(request, env, ctx, refFromPath(url.pathname, known));
    }
    const upstream = await fetch(`${ORIGIN}${url.pathname}${url.search}`, {
      method: request.method,
      redirect: "manual",
      // Short, so a release baked into the page shows within minutes.
      cf: { cacheEverything: true, cacheTtl: 300 },
    });
    const response = new Response(upstream.body, upstream);
    // GitHub's own redirects (/img → /img/) point at github.io; keep them here.
    const location = response.headers.get("Location");
    if (location) response.headers.set("Location", rewriteLocation(location));
    withSecurityHeaders(response.headers);
    return response;
  },
};

/**
 * The latest setup under a name that carries where the link came from — the
 * installer writes its own name down and the app reads the source out of it. A
 * redirect would not do: then GitHub's name would win. Each real download adds
 * one to its link's count for the day; nothing else about the request is kept.
 */
async function download(request: Request, env: Env, ctx: ExecutionContext, ref: string): Promise<Response> {
  let release: { version: string; url: string };
  let file: Response;
  try {
    // The release the page itself shows, baked in after every release. Only
    // a setup of our own releases is fetched, under a plain version number.
    const baked = await fetch(`${ORIGIN}/release.json`, { cf: { cacheEverything: true, cacheTtl: 300 } });
    if (!baked.ok) throw new Error(`release.json: ${baked.status}`);
    const checked = validRelease(await baked.json());
    if (!checked) throw new Error("release.json: not a release of ours");
    release = checked;
    file = await fetch(release.url, { method: request.method });
    if (!file.ok) throw new Error(`setup: ${file.status}`);
  } catch (err) {
    console.error(err);
    return Response.redirect(RELEASES, 302);
  }
  const address = request.headers.get("CF-Connecting-IP") ?? "unknown";
  if (
    request.method === "GET" &&
    !isBot(request.headers.get("User-Agent")) &&
    (await env.DOWNLOAD_LIMIT.limit({ key: addressKey(address) })).success
  ) {
    ctx.waitUntil(count(env, ref).catch((err) => console.error("download count", err)));
  }
  const headers = new Headers({
    "Content-Type": "application/octet-stream",
    "Content-Disposition": `attachment; filename="${downloadName(release.version, ref)}"`,
    "Cache-Control": "no-store",
  });
  const length = file.headers.get("Content-Length");
  if (length) headers.set("Content-Length", length);
  return new Response(request.method === "HEAD" ? null : file.body, { headers });
}

/**
 * One more download of `ref` today — while the day's counted downloads, all
 * links together, are under the ceiling (`downloadCeiling`). Past it nothing
 * is written, and the day's total stays where it stopped.
 */
async function count(env: Env, ref: string): Promise<void> {
  const counted = await env.DB.prepare(
    `INSERT INTO downloads (day, ref, count)
     SELECT ?1, ?2, 1 WHERE (SELECT COALESCE(SUM(count), 0) FROM downloads WHERE day = ?1) < ?3
     ON CONFLICT (day, ref) DO UPDATE SET count = count + 1`,
  )
    .bind(utcDay(Date.now()), ref, downloadCeiling(env.DOWNLOADS_PER_DAY))
    .run();
  if (counted.meta.changes !== 1) return;
  await env.DB.prepare("INSERT INTO download_totals (ref, count) VALUES (?, 1) ON CONFLICT(ref) DO UPDATE SET count = count + 1")
    .bind(ref)
    .run();
}

/**
 * The number on the page: real downloads through clippiboy.com/get/…, all links
 * together. GitHub's own counter would add every auto-update, since the updater
 * fetches the same installer. Kept at the edge for ten minutes, so visitors
 * don't each ask the database.
 */
async function stats(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const cache = caches.default;
  const key = new Request(new URL("/stats.json", HOME).toString());
  const kept = await cache.match(key);
  if (kept) return kept;
  let downloads = 0;
  try {
    const row = await env.DB.prepare("SELECT COALESCE(SUM(count), 0) AS total FROM download_totals").first<{ total: number }>();
    downloads = row?.total ?? 0;
  } catch (err) {
    console.error(err);
    return new Response(JSON.stringify({ downloads: 0 }), {
      headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
    });
  }
  const response = new Response(JSON.stringify({ downloads }), {
    headers: { "Content-Type": "application/json", "Cache-Control": "public, max-age=600" },
  });
  if (request.method === "GET") ctx.waitUntil(cache.put(key, response.clone()));
  return response;
}
