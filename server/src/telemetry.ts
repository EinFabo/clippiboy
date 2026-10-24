// Anonymous usage pings and download counts. One ping a day per install: a
// random id, the version, where the download came from and a few counts — no
// account, no content, no address. Rows are kept 400 days.
//
// No imports from the rest of the server, so `node --test` can load this file
// as it is (util.ts uses syntax type stripping does not cover).

/** A download link's source: `clippiboy.com/get/<ref>`. */
export const REF_PATTERN = /^[a-z0-9-]{1,32}$/;

/** Answers to the setup's "Where did you hear about ClippiBoy?". */
export const HEARD = ["tiktok", "youtube", "discord", "friend", "reddit", "other"] as const;

/** A ping's body is read up to this many bytes; past it, it is refused. */
export const MAX_PING_BYTES = 1024;

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const MAX_COUNT = 10_000;
const MAX_FEATURES = 63;
const KEEP_DAYS = 400;
const DAY_MS = 24 * 60 * 60 * 1000;

export type Ping = {
  id: string;
  version: string;
  source: string | null;
  heard: string | null;
  clips: number;
  shots: number;
  recordings: number;
  features: number;
};

const isCount = (value: unknown, max: number): value is number =>
  Number.isInteger(value) && (value as number) >= 0 && (value as number) <= max;

/** The ping as sent, or null if any field is off. */
export function cleanPing(raw: unknown): Ping | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  const source = p.source ?? null;
  const heard = p.heard ?? null;
  if (typeof p.id !== "string" || !UUID_V4.test(p.id)) return null;
  if (typeof p.version !== "string" || p.version.length === 0 || p.version.length > 32) return null;
  if (source !== null && (typeof source !== "string" || !REF_PATTERN.test(source))) return null;
  if (heard !== null && !(HEARD as readonly unknown[]).includes(heard)) return null;
  if (!isCount(p.clips, MAX_COUNT) || !isCount(p.shots, MAX_COUNT) || !isCount(p.recordings, MAX_COUNT)) return null;
  if (!isCount(p.features, MAX_FEATURES)) return null;
  return {
    id: p.id,
    version: p.version,
    source: source as string | null,
    heard: heard as string | null,
    clips: p.clips,
    shots: p.shots,
    recordings: p.recordings,
    features: p.features,
  };
}

export function utcDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/** The first day still kept; anything before it goes. */
export function cutoffDay(ms: number, days = KEEP_DAYS): string {
  return utcDay(ms - days * DAY_MS);
}

/**
 * `/get/<ref>` → the ref; anything that is not a valid ref counts as the site.
 * With `known`, so does a ref not in it: a made-up link adds no row of its own.
 */
export function refFromPath(pathname: string, known?: readonly string[]): string {
  const ref = pathname.replace(/^\/get\/?/, "").replace(/\/+$/, "").toLowerCase();
  if (!REF_PATTERN.test(ref)) return "site";
  return !known || known.includes(ref) ? ref : "site";
}

export function isBot(userAgent: string | null): boolean {
  return userAgent !== null && /bot|crawler|spider|facebookexternalhit|whatsapp|embedly|preview/i.test(userAgent);
}

/** The installer reads its source back out of this name. */
export function downloadName(version: string, ref: string): string {
  return `ClippiBoy_${version}_x64-setup.${ref}.exe`;
}

/** `text` is the body as the router read it, null when it was over MAX_PING_BYTES. */
export async function handlePing(text: string | null, env: Env): Promise<Response> {
  const bad = () => Response.json({ error: "Bad ping." }, { status: 400 });
  if (text === null) return bad();
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return bad();
  }
  const ping = cleanPing(raw);
  if (!ping) return bad();

  const day = utcDay(Date.now());
  await env.DB.batch([
    env.DB.prepare(
      `INSERT INTO installs (id, source, heard, first_day, last_day, version) VALUES (?1, ?2, ?3, ?4, ?4, ?5)
       ON CONFLICT(id) DO UPDATE SET source = COALESCE(installs.source, excluded.source),
         heard = COALESCE(installs.heard, excluded.heard), last_day = excluded.last_day, version = excluded.version`,
    ).bind(ping.id, ping.source, ping.heard, day, ping.version),
    env.DB.prepare(
      `INSERT INTO install_days (id, day, clips, shots, recordings, features) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
       ON CONFLICT(id, day) DO UPDATE SET clips = clips + excluded.clips, shots = shots + excluded.shots,
         recordings = recordings + excluded.recordings, features = features | excluded.features`,
    ).bind(ping.id, day, ping.clips, ping.shots, ping.recordings, ping.features),
  ]);
  return Response.json({ ok: true });
}

/** Run by the hourly cron. */
export async function sweepUsage(env: Env, now = Date.now()): Promise<void> {
  const cutoff = cutoffDay(now);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM install_days WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM downloads WHERE day < ?").bind(cutoff),
    env.DB.prepare("DELETE FROM installs WHERE last_day < ?").bind(cutoff),
  ]);
}
