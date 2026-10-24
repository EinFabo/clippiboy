import { addressKey } from "../src/limits";
import { downloadName, isBot, refFromPath, utcDay } from "../src/telemetry";

const ORIGIN = "https://einfabo.github.io/clippiboy";
const RELEASES = "https://github.com/EinFabo/clippiboy/releases/latest";
const HOME = "https://clippiboy.com";

interface Env {
  API: Fetcher;
  DB: D1Database;
  DOWNLOAD_LIMIT: RateLimit;
  /** The refs campaign links use, comma-separated; any other counts as the site. */
  DOWNLOAD_REFS: string;
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
    // A share link's page; the video on it loads from api.clippiboy.com directly.
    if (/^\/c\/[a-z0-9]+$/.test(url.pathname)) {
      return env.API.fetch(request);
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
    if (location?.startsWith(ORIGIN)) {
      response.headers.set("Location", HOME + location.slice(ORIGIN.length));
    } else if (location?.startsWith("/clippiboy")) {
      response.headers.set("Location", location.slice("/clippiboy".length) || "/");
    }
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
  let release: { version: string; setup: { url: string } };
  let file: Response;
  try {
    // The release the page itself shows, baked in after every release.
    const baked = await fetch(`${ORIGIN}/release.json`, { cf: { cacheEverything: true, cacheTtl: 300 } });
    if (!baked.ok) throw new Error(`release.json: ${baked.status}`);
    release = await baked.json();
    file = await fetch(release.setup.url, { method: request.method });
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
    ctx.waitUntil(
      env.DB.prepare(
        "INSERT INTO downloads (day, ref, count) VALUES (?, ?, 1) ON CONFLICT(day, ref) DO UPDATE SET count = count + 1",
      )
        .bind(utcDay(Date.now()), ref)
        .run(),
    );
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
