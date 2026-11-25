import {
  authenticate,
  departStatement,
  pair,
  related,
  toMe,
  toUser,
  SESSION_IDLE_MS,
  tokenHash,
  upsertDiscordUser,
  userByCode,
  userById,
  userByName,
  type DiscordUser,
  type User,
} from "./db";
import { CHALLENGE_PATTERN, LOGIN_TTL_MS, pageCsp, signState, statesIn, verifyState, withoutState, withState } from "./login";
import { base64url, body, escapeHtml, HttpError, json, normalizeCode, randomToken, sameText, sha256 } from "./util";
import {
  applySuspensions,
  createShare,
  deleteShare,
  deleteSharesOf,
  putPoster,
  reportShare,
  shareAvatar,
  shareFile,
  sharePage,
  shareQuota,
  sweep,
} from "./links";
import { addressKey, dialAs, readLimited, writeBudget } from "./limits";
import { CLOSE_DELETING } from "./hubrules";
import { departedCutoff, fingerprint } from "./departed";
import { handlePing, MAX_PING_BYTES, sweepUsage, utcDay } from "./telemetry";
import {
  NOBODY,
  noticeFor,
  REQUEST_COOLDOWN_MS,
  requestRule,
  type PairStatus,
  type RequestOutcome,
} from "./friendrules";
import { clearInviteCookie, cookieValue, INVITE_COOKIE, inviteTarget, verifyInvite } from "./invite";
import { invitePage, inviteSecret, shareInvite } from "./invitepages";
import {
  fresh,
  grantedWebhook,
  isKey,
  isState,
  seal,
  WEBHOOK_COOKIE,
  WEBHOOK_SCOPE,
  WEBHOOK_TTL_MS,
  webhookLink,
} from "./webhook";

export { Hub } from "./hub";

const STATE_COOKIE = "cb_login";
const DAY_MS = 24 * 60 * 60 * 1000;

export default {
  async fetch(request, env): Promise<Response> {
    try {
      return await route(request, env);
    } catch (err) {
      if (err instanceof HttpError) return json({ error: err.message }, err.status);
      console.error(err);
      return json({ error: "Something went wrong on the server." }, 500);
    }
  },
  async scheduled(_controller, env, ctx) {
    // Each on its own: one failing must not cut the others short.
    ctx.waitUntil(
      Promise.allSettled([
        sweep(env),
        sweepUsage(env),
        applySuspensions(env),
        // Here rather than on every /auth/start: there it scanned the whole
        // table each time, and a flood of starts made that quadratic. Rows a
        // little past their time are harmless — callback and exchange check
        // the age themselves.
        env.DB.prepare("DELETE FROM logins WHERE created_at < ?").bind(Date.now() - LOGIN_TTL_MS).run(),
        // Channel connections nobody finished: only their keys, never a webhook.
        env.DB.prepare("DELETE FROM webhook_logins WHERE created_at < ?").bind(Date.now() - WEBHOOK_TTL_MS).run(),
        // Yesterday's count stays readable a day; older ones are done.
        env.DB.prepare("DELETE FROM anon_budget WHERE day < ?").bind(utcDay(Date.now() - DAY_MS)).run(),
        // What a deleted account left behind is kept for seven days, a
        // suspension for as long as it stands (`departedKept`) — but its link
        // times only for the seven days, as the privacy page says. Only rows
        // that still hold some are written.
        env.DB.prepare("DELETE FROM departed WHERE left_at <= ? AND banned_at IS NULL")
          .bind(departedCutoff(Date.now()))
          .run(),
        env.DB.prepare(
          "UPDATE departed SET link_times = '[]' WHERE left_at <= ? AND banned_at IS NOT NULL AND link_times != '[]'",
        )
          .bind(departedCutoff(Date.now()))
          .run(),
        // Sessions unused for that long no longer sign anyone in; their rows go too.
        env.DB.prepare("DELETE FROM sessions WHERE last_used < ?").bind(Date.now() - SESSION_IDLE_MS).run(),
        // A pair's rest after a withdrawn or declined request, once it is over.
        env.DB.prepare("DELETE FROM request_cooldowns WHERE ends_at <= ?").bind(Date.now()).run(),
      ]).then((outcomes) => {
        for (const outcome of outcomes) {
          if (outcome.status === "rejected") console.error("cron", outcome.reason);
        }
      }),
    );
  },
} satisfies ExportedHandler<Env>;

async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  const method = request.method;

  if (path === "/" && method === "GET") return new Response("ClippiBoy friends server");
  if (path === "/auth/start" && method === "GET") {
    if (!(await underLimit(request, env, "auth"))) return page("Too many sign-ins", "Wait a minute, then try again from ClippiBoy.");
    return authStart(request, url, env);
  }
  if (path === "/auth/callback" && method === "GET") {
    // Each one asks Discord, so it is counted per address like the start.
    // Nothing is spent yet: the page can be reloaded a minute later.
    if (!(await underLimit(request, env, "callback"))) return page("Too many sign-ins", "Wait a minute, then reload this page.");
    return authCallback(request, url, env);
  }
  // A suspended account can sign in no more, so it is deleted from the page
  // that says it is suspended.
  if (path === "/auth/delete" && method === "POST") {
    if (!(await underLimit(request, env, "delete"))) return page("Slow down", "Wait a minute, then try again.");
    return deleteSuspended(request, env);
  }
  if (path === "/auth/exchange" && method === "POST") {
    if (!(await underLimit(request, env, "exchange"))) return json({ error: "Too many sign-ins — wait a minute." }, 429);
    return authExchange(request, env);
  }

  // "Post clips to a Discord channel": the browser half, before the session check.
  if (path === "/webhook/start" && method === "GET") {
    if (!(await underLimit(request, env, "webhook"))) return page("Slow down", "Wait a minute, then try again from ClippiBoy.");
    return webhookStart(request, url, env);
  }
  if (path === "/webhook/callback" && method === "GET") return webhookCallback(request, url, env);

  // Share links are for anyone who has the link.
  const shared = path.match(/^\/c\/([a-z0-9]+)(?:\.(mp4|jpg))?$/);
  if (shared && (method === "GET" || method === "HEAD")) {
    const [, id, kind] = shared;
    return kind ? shareFile(request, env, id, kind as "mp4" | "jpg") : sharePage(env, id);
  }
  // The uploader's picture on that page, without Discord's link (avatar.ts).
  const pictured = path.match(/^\/c\/([a-z0-9]+)\/avatar$/);
  if (pictured && (method === "GET" || method === "HEAD")) return shareAvatar(request, env, pictured[1]);
  const reported = path.match(/^\/c\/([a-z0-9]+)\/report$/);
  if (reported && method === "POST") return reportShare(request, env, reported[1]);

  // Invites: someone's own link, and "Add <name> as friend" there and on a share page.
  const invite = path.match(/^\/i\/([A-Za-z0-9-]{8,9})(\/add)?$/);
  if (invite && (method === "GET" || method === "HEAD")) {
    // Each one looks a friend code up: counted per address, like sign-in.
    if (!(await underLimit(request, env, "invite"))) return page("Slow down", "Wait a minute, then open the link again.");
    return invitePage(request, env, invite[1], !!invite[2]);
  }
  const shareAdd = path.match(/^\/c\/([a-z0-9]+)\/add$/);
  if (shareAdd && (method === "GET" || method === "HEAD")) return shareInvite(request, env, shareAdd[1]);

  // Anonymous on purpose: a ping never carries the account.
  if (path === "/t/ping" && method === "POST") {
    if (!(await underLimit(request, env, "ping"))) return json({ error: "Too many pings." }, 429);
    // Null when it is too big: handlePing refuses it without spending the day's ceiling.
    const text = await readLimited(request, MAX_PING_BYTES);
    // Past the ceiling the app keeps its counts and sends them with a later ping.
    if (text !== null && !(await withinCeiling(env, "ping"))) return json({ error: "Too many pings." }, 429);
    return handlePing(text, env);
  }

  // Each connection wakes the user's hub, a Durable Object request on the
  // free plan's daily 100,000 for everybody: counted per account below, ten
  // a minute — the app dials once after a drop and backs off from 1 s to a
  // minute. Per address only a dial that fails to sign in (`dialAs`), so
  // others behind the same address are never kept out.
  const socket = path === "/ws" && method === "GET";
  const me = socket
    ? await dialAs(
        () => authenticate(request, env.DB),
        () => underLimit(request, env, "ws"),
        () => new HttpError(429, "Too many connections — wait a minute."),
      )
    : await authenticate(request, env.DB);

  // Everything that changes something is counted per user; reads and the
  // sign-out are not. That keeps a script holding one session from flooding
  // the friends' sockets with notices, but 30 a minute is still tens of
  // thousands of writes a day: the routes that write the most for the least
  // count against a tighter limit of their own on top (`writeBudget`), and
  // failed uploads past a few free ones count against the week (links.ts). What one
  // account can still write in a day is about half the database's free daily
  // writes — see "Limits" in server/README.md.
  if (method !== "GET" && !(path === "/auth/logout" && method === "POST")) {
    if (!(await env.USER_LIMIT.limit({ key: me })).success) {
      throw new HttpError(429, "Slow down — try again in a minute.");
    }
    const budget = writeBudget(method, path);
    if (budget) await spend(env, me, budget);
  }

  if (path === "/webhook/begin" && method === "POST") return webhookBegin(request, url, env);
  if (path === "/webhook/cancel" && method === "POST") return webhookCancel(request, env);
  if (path === "/shares/quota" && method === "GET") return shareQuota(env, me);
  if (path === "/shares" && method === "POST") return createShare(request, env, me);
  const share = path.match(/^\/shares\/([a-z0-9]+)(\/poster)?$/);
  if (share) {
    const [, id, isPoster] = share;
    if (isPoster && method === "PUT") return putPoster(request, env, me, id);
    if (!isPoster && method === "DELETE") return deleteShare(env, me, id);
  }

  if (socket) {
    // Same binding as the routes anyone may call, under a key of the account's own.
    if (!(await env.ANON_LIMIT.limit({ key: `ws-user:${me}` })).success) {
      throw new HttpError(429, "Too many connections — wait a minute.");
    }
    const headers = new Headers(request.headers);
    headers.set("X-User", me);
    return env.HUB.getByName(me).fetch(new Request(request, { headers }));
  }
  if (path === "/auth/logout" && method === "POST") {
    const token = request.headers.get("Authorization")!.slice(7).trim();
    const [, left] = await env.DB.batch([
      env.DB.prepare("DELETE FROM sessions WHERE token_hash = ?").bind(await tokenHash(token)),
      // A session still able to sign in (`authenticate`), on another device.
      env.DB.prepare("SELECT 1 FROM sessions WHERE user_id = ? AND last_used >= ? LIMIT 1").bind(
        me,
        Date.now() - SESSION_IDLE_MS,
      ),
    ]);
    // The last one: an open connection must not outlive it. The sign-out
    // itself is done either way.
    if (left.results.length === 0) {
      await env.HUB.getByName(me)
        .signedOut(me)
        .catch((err) => console.error("sign-out", me, err));
    }
    return json({ ok: true });
  }
  if (path === "/me" && method === "GET") return json(await loadMe(env, me));
  if (path === "/me" && method === "PATCH") {
    const { allowRequests } = await body<{ allowRequests: boolean }>(request);
    if (typeof allowRequests === "boolean") {
      // Only a real change writes.
      await env.DB.prepare("UPDATE users SET allow_requests = ? WHERE id = ? AND allow_requests != ?")
        .bind(allowRequests ? 1 : 0, me, allowRequests ? 1 : 0)
        .run();
    }
    return json(await loadMe(env, me));
  }
  if (path === "/me" && method === "DELETE") return deleteAccount(env, me);
  if (path === "/friends" && method === "GET") return json(await listFriends(env, me));
  if (path === "/friends/request" && method === "POST") {
    const { query } = await body<{ query: string }>(request);
    return json(await sendRequest(env, me, typeof query === "string" ? query : ""));
  }

  const friend = path.match(/^\/friends\/([0-9a-f-]{36})(\/accept)?$/);
  if (friend) {
    const [, other, accept] = friend;
    if (accept && method === "POST") return json(await acceptRequest(env, me, other));
    if (!accept && method === "DELETE") return json(await removeFriend(env, me, other));
  }
  const block = path.match(/^\/blocks\/([0-9a-f-]{36})$/);
  if (block) {
    const other = block[1];
    if (method === "POST") return json(await blockUser(env, me, other));
    if (method === "DELETE") {
      await env.DB.prepare("DELETE FROM blocks WHERE blocker = ? AND blocked = ?").bind(me, other).run();
      return json({ ok: true });
    }
  }
  throw new HttpError(404, "Not found.");
}

/** Counts one call against one of the user's tighter limits, or refuses it. */
async function spend(env: Env, me: string, budget: "share" | "connect" | "friend"): Promise<void> {
  const limit = { share: env.SHARE_LIMIT, connect: env.CONNECT_LIMIT, friend: env.FRIEND_LIMIT }[budget];
  if (!(await limit.limit({ key: me })).success) throw new HttpError(429, "Slow down — try again in a minute.");
}

/**
 * Within the per-address limit for routes anyone may call? Each route counts on
 * its own, so a flood of pings cannot lock the same address out of signing in.
 * An IPv6 address counts as its /48 (`addressKey`).
 */
async function underLimit(
  request: Request,
  env: Env,
  route: "auth" | "callback" | "ping" | "exchange" | "invite" | "webhook" | "delete" | "ws",
): Promise<boolean> {
  const address = request.headers.get("CF-Connecting-IP") ?? "unknown";
  return (await env.ANON_LIMIT.limit({ key: `${route}:${addressKey(address)}` })).success;
}

/**
 * Within today's ceiling for pings? Counted across all addresses, one per call,
 * after the per-address limit: many addresses together still cannot spend the
 * database's daily writes. The counter stops at the ceiling, so a refused call
 * writes nothing. Sign-in has no such ceiling: one address could use it up and
 * lock everybody else out, so it is held only by the per-address limit.
 */
async function withinCeiling(env: Env, route: "ping"): Promise<boolean> {
  const configured = Number(env.ANON_PINGS_PER_DAY);
  const ceiling = Number.isInteger(configured) && configured > 0 ? configured : 6000;
  const result = await env.DB.prepare(
    `INSERT INTO anon_budget (day, route, count) VALUES (?1, ?2, 1)
     ON CONFLICT(day, route) DO UPDATE SET count = count + 1 WHERE count < ?3`,
  )
    .bind(utcDay(Date.now()), route, ceiling)
    .run();
  return result.meta.changes === 1;
}

// --- Login --------------------------------------------------------------------
//
// The app opens /auth/start in the browser with the hash of a verifier it keeps.
// Discord sends the browser back to /auth/callback, which hands the app a
// one-time code through clippiboy://auth. Only the app that knows the verifier
// can trade that code for a session — another program that grabs the link
// cannot. The cookie ties the callback to the browser that started the login,
// so nobody can send someone a half-finished login of their own to complete.
//
// The start writes nothing: the state carries the challenge and the start
// time, signed (`signState` in login.ts), so a flood of starts costs the
// database nothing. The sign-in's row is written once Discord has said yes,
// and stays for the state's ten minutes, so a state signs in once.

/** The cookie holding the browser's sign-in states, or clearing it when there are none. */
function stateCookie(value: string): string {
  return value
    ? `${STATE_COOKIE}=${value}; Path=/auth; Max-Age=600; HttpOnly; Secure; SameSite=Lax`
    : `${STATE_COOKIE}=; Path=/auth; Max-Age=0; HttpOnly; Secure; SameSite=Lax`;
}

/**
 * A state Discord turned down is spent: noted in this data centre's cache
 * for the state's lifetime, which writes nothing anywhere. Only best effort —
 * another data centre, or workers.dev, which has no cache, does not see it —
 * so the browser's cookie forgets the state too, and the callback's limit per
 * address holds whatever is left: a fresh state is as cheap to get as a
 * spent one is to retry. A row per failure would have been a write anybody
 * could cause, which is what the start no longer does.
 */
function spentKey(url: URL, state: string): Request {
  return new Request(`${url.origin}/auth/spent/${state}`);
}

async function wasSpent(url: URL, state: string): Promise<boolean> {
  try {
    return (await caches.default.match(spentKey(url, state))) !== undefined;
  } catch {
    return false;
  }
}

async function markSpent(url: URL, state: string): Promise<void> {
  try {
    await caches.default.put(
      spentKey(url, state),
      new Response("spent", { headers: { "Cache-Control": `max-age=${LOGIN_TTL_MS / 1000}` } }),
    );
  } catch (err) {
    console.error("spent state", err);
  }
}

async function authStart(request: Request, url: URL, env: Env): Promise<Response> {
  const challenge = url.searchParams.get("challenge") ?? "";
  if (!CHALLENGE_PATTERN.test(challenge)) throw new HttpError(400, "Missing challenge.");
  if (!env.DISCORD_CLIENT_ID || !env.DISCORD_CLIENT_SECRET) throw new HttpError(503, "Discord login is not configured yet.");
  const state = await signState(env.DISCORD_CLIENT_SECRET, challenge, Date.now());
  const authorize = new URL("https://discord.com/oauth2/authorize");
  authorize.searchParams.set("client_id", env.DISCORD_CLIENT_ID);
  authorize.searchParams.set("response_type", "code");
  authorize.searchParams.set("redirect_uri", `${url.origin}/auth/callback`);
  authorize.searchParams.set("scope", "identify");
  authorize.searchParams.set("state", state);
  return new Response(null, {
    status: 302,
    headers: {
      Location: authorize.toString(),
      "Set-Cookie": stateCookie(withState(statesIn(request.headers.get("Cookie"), STATE_COOKIE), state)),
    },
  });
}

async function authCallback(request: Request, url: URL, env: Env): Promise<Response> {
  const state = url.searchParams.get("state") ?? "";
  const code = url.searchParams.get("code");
  const states = statesIn(request.headers.get("Cookie"), STATE_COOKIE);
  if (!code) return page("Sign-in cancelled", "You can close this tab and try again from ClippiBoy.");
  if (!states.some((s) => sameText(s, state))) {
    return page("Sign-in expired", "Start the sign-in again from ClippiBoy, in this browser.");
  }
  const login = await verifyState(env.DISCORD_CLIENT_SECRET, state, Date.now());
  // Signed in with once already (its row stays for the state's lifetime), or
  // turned down by Discord.
  if (
    !login ||
    (await wasSpent(url, state)) ||
    (await env.DB.prepare("SELECT 1 FROM logins WHERE state = ?").bind(state).first()) !== null
  ) {
    return page("Sign-in expired", "Start the sign-in again from ClippiBoy.");
  }
  // From here a failure spends the state: Discord's code is used up, and the
  // same state must not ask Discord again.
  const failed = async (text: string) => {
    await markSpent(url, state);
    return page("Discord said no", text, undefined, [stateCookie(withoutState(states, state))]);
  };

  let tokenResponse: Response;
  try {
    tokenResponse = await fetch("https://discord.com/api/oauth2/token", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        code,
        redirect_uri: `${url.origin}/auth/callback`,
        client_id: env.DISCORD_CLIENT_ID,
        client_secret: env.DISCORD_CLIENT_SECRET,
      }),
    });
  } catch (err) {
    console.error("discord token", err);
    return failed("Discord did not confirm the sign-in. Try again from ClippiBoy.");
  }
  if (!tokenResponse.ok) {
    console.error("discord token", tokenResponse.status, await tokenResponse.text());
    return failed("Discord did not confirm the sign-in. Try again from ClippiBoy.");
  }
  const { access_token } = (await tokenResponse.json().catch(() => ({}))) as { access_token?: unknown };
  if (typeof access_token !== "string" || !access_token) {
    return failed("Discord did not confirm the sign-in. Try again from ClippiBoy.");
  }
  let userResponse: Response;
  try {
    userResponse = await fetch("https://discord.com/api/users/@me", {
      headers: { Authorization: `Bearer ${access_token}` },
    });
  } catch (err) {
    console.error("discord profile", err);
    return failed("Could not read your Discord profile.");
  }
  if (!userResponse.ok) return failed("Could not read your Discord profile.");
  const discord = (await userResponse.json()) as DiscordUser;
  const userId = await upsertDiscordUser(env.DB, discord, await departedHash(env, discord.id));
  const banned = await env.DB.prepare("SELECT 1 FROM users WHERE id = ? AND banned_at IS NOT NULL").bind(userId).first();

  // The sign-in's one row, now that Discord has said yes, claimed by its
  // state: a second callback with it finds it taken. Its ten minutes run from
  // the start, as the state's do.
  const oneTime = randomToken(24);
  const claimed = await env.DB.prepare(
    "INSERT INTO logins (state, challenge, code, user_id, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (state) DO NOTHING",
  )
    .bind(state, login.challenge, oneTime, userId, login.issuedAt)
    .run();
  if (claimed.meta.changes !== 1) return page("Sign-in expired", "Start the sign-in again from ClippiBoy.");
  if (banned) {
    // No session, but the account may still be deleted: the code is good for
    // that alone (`deleteSuspended`), from this browser and within the sign-in's time.
    return page(
      "Account suspended",
      "This ClippiBoy account has been suspended. You can still delete it, with its friends and links.",
      undefined,
      [],
      { action: "/auth/delete", code: oneTime, label: "Delete account" },
    );
  }

  const invited = await inviteAtSignIn(request, env, userId);
  const back = `clippiboy://auth?code=${oneTime}`;
  const cookies = [stateCookie("")];
  // Spent once read, whatever came of it: one request per invite.
  if (invited.present) cookies.push(clearInviteCookie());
  const friend =
    invited.outcome === "accept"
      ? `You and ${invited.name} are friends now. `
      : invited.outcome === "send"
        ? `Friend request sent to ${invited.name}. `
        : "";
  return page("Signed in", `${friend}Back to ClippiBoy — you can close this tab.`, back, cookies);
}

/**
 * The friend request an invite cookie asks for (invite.ts): sent at sign-in,
 * from the one signing in to whoever invited them — only for an account made
 * after the click, and never to a suspended inviter (`inviteTarget`) — by the
 * same rule as the Add field: blocks, "don't take requests", an existing pair
 * all hold. Counted
 * against the user's limit like any other request. A forged, expired or
 * refused invite is dropped without a word; it never fails the sign-in.
 */
async function inviteAtSignIn(
  request: Request,
  env: Env,
  me: string,
): Promise<{ present: boolean; outcome: RequestOutcome["kind"] | null; name: string }> {
  const token = cookieValue(request.headers.get("Cookie"), INVITE_COOKIE);
  const none = { present: token !== null, outcome: null, name: "" };
  const secret = inviteSecret(env);
  if (!token || !secret) return none;
  try {
    const invite = await verifyInvite(secret, token, Date.now());
    if (!invite) return none;
    const [self, target] = await Promise.all([userById(env.DB, me), userById(env.DB, invite.inviter)]);
    const to = inviteTarget(
      invite,
      { id: me, createdAt: self?.created_at ?? 0 },
      target ? { banned: target.banned_at != null } : null,
    );
    if (!to || !target) return none;
    if (!(await env.USER_LIMIT.limit({ key: me })).success) return none;
    const outcome = await requestTo(env, me, target, true);
    return { present: true, outcome: outcome.kind, name: target.display_name };
  } catch (err) {
    console.error("invite", err);
    return none;
  }
}

/**
 * "Delete account" on the suspended page. The one-time code from that page,
 * from the browser that signed in (its state cookie), within the sign-in's
 * ten minutes, and only while the account is suspended — any other is deleted
 * in the app. The suspension outlives the account (departed.ts), so deleting
 * it is no way around it.
 */
async function deleteSuspended(request: Request, env: Env): Promise<Response> {
  const form = new URLSearchParams((await readLimited(request, 1024)) ?? "");
  const code = form.get("code") ?? "";
  const login = code
    ? await env.DB.prepare("SELECT state, user_id, created_at FROM logins WHERE code = ?")
        .bind(code)
        .first<{ state: string; user_id: string | null; created_at: number }>()
    : null;
  const states = statesIn(request.headers.get("Cookie"), STATE_COOKIE);
  if (
    !login?.user_id ||
    Date.now() - login.created_at > LOGIN_TTL_MS ||
    !states.some((s) => sameText(s, login.state))
  ) {
    return page("Page expired", "Sign in again from ClippiBoy to delete the account.");
  }
  const suspended = await env.DB.prepare("SELECT 1 FROM users WHERE id = ? AND banned_at IS NOT NULL")
    .bind(login.user_id)
    .first();
  if (!suspended) return page("Not suspended", "Delete the account in ClippiBoy, under Settings → Friends.");
  // Without the pepper nothing outlives the account, the suspension included.
  if (!env.DEPARTED_PEPPER) return page("Not possible right now", "Try again later, or write to privacy@clippiboy.com.");
  // Spent once it is used; the row stays until its time is up, so its state
  // cannot sign in again meanwhile.
  await env.DB.prepare("UPDATE logins SET code = NULL, user_id = NULL WHERE state = ?").bind(login.state).run();
  try {
    await deleteAccount(env, login.user_id);
  } catch (err) {
    console.error("delete suspended", login.user_id, err);
    return page("Something went wrong", "The account is not deleted yet. Sign in again from ClippiBoy and try once more.");
  }
  return page("Account deleted", "Your ClippiBoy account is gone. You can close this tab.", undefined, [stateCookie("")]);
}

async function authExchange(request: Request, env: Env): Promise<Response> {
  const { code, verifier } = await body<{ code: string; verifier: string }>(request);
  // Strings only: anything else went to the database as it was, and ended in a 500.
  if (typeof code !== "string" || typeof verifier !== "string" || !code || !verifier) {
    throw new HttpError(400, "Missing code or verifier.");
  }
  const login = await env.DB.prepare("SELECT state, challenge, user_id, created_at FROM logins WHERE code = ?")
    .bind(code)
    .first<{ state: string; challenge: string; user_id: string; created_at: number }>();
  // Spent on the first try, right or wrong. The row itself stays until its
  // time is up (the hourly cron), so its state cannot sign in again meanwhile.
  if (login) {
    await env.DB.prepare("UPDATE logins SET code = NULL, user_id = NULL WHERE state = ?").bind(login.state).run();
  }
  if (!login || Date.now() - login.created_at > LOGIN_TTL_MS) {
    throw new HttpError(400, "That sign-in has expired.");
  }
  if (!sameText(base64url(await sha256(verifier)), login.challenge)) {
    throw new HttpError(400, "That sign-in belongs to someone else.");
  }
  const token = randomToken(32);
  const now = Date.now();
  await env.DB.prepare("INSERT INTO sessions (token_hash, user_id, created_at, last_used) VALUES (?, ?, ?, ?)")
    .bind(await tokenHash(token), login.user_id, now, now)
    .run();
  return json({ token, me: await loadMe(env, login.user_id) });
}

/** Hands an access token back to Discord; a failure only costs the token's own expiry. */
async function revokeToken(env: Env, token: string): Promise<void> {
  try {
    const response = await fetch("https://discord.com/api/oauth2/token/revoke", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        token,
        token_type_hint: "access_token",
        client_id: env.DISCORD_CLIENT_ID,
        client_secret: env.DISCORD_CLIENT_SECRET,
      }),
    });
    if (!response.ok) console.error("discord revoke", response.status);
  } catch (err) {
    console.error("discord revoke", err);
  }
}

// --- Posting to a channel -----------------------------------------------------
//
// The app sends a key of its own with its session (begin) and opens the start
// URL it gets back in the browser. Discord asks for a server and a channel and
// comes back to the callback, which trades the code for the webhook, seals it
// with the app's key (webhook.ts) and hands it over in clippiboy://webhook. The
// webhook is never written anywhere here; the row holds only the key, and is
// spent by the callback. The cookie ties the callback to the browser that
// started, as with sign-in.

async function webhookBegin(request: Request, url: URL, env: Env): Promise<Response> {
  const { key } = await body<{ key: string }>(request);
  if (!isKey(key)) throw new HttpError(400, "Missing key.");
  if (!env.DISCORD_CLIENT_ID) throw new HttpError(503, "Discord is not configured yet.");
  const state = randomToken(24);
  await env.DB.prepare("INSERT INTO webhook_logins (state, key, created_at) VALUES (?, ?, ?)")
    .bind(state, key, Date.now())
    .run();
  return json({ url: `${url.origin}/webhook/start?state=${state}`, state });
}

/** "Cancel" in the app: the start page refuses the tab from then on. */
async function webhookCancel(request: Request, env: Env): Promise<Response> {
  const { state } = await body<{ state: string }>(request);
  if (isState(state)) await env.DB.prepare("DELETE FROM webhook_logins WHERE state = ?").bind(state).run();
  return json({ ok: true });
}

async function webhookStart(request: Request, url: URL, env: Env): Promise<Response> {
  const state = url.searchParams.get("state");
  const expired = page("Link expired", "Press Connect channel in ClippiBoy again.");
  if (!isState(state)) return expired;
  // Once only: the link opened a second time, in this browser or another, goes nowhere.
  const started = await env.DB.prepare(
    "UPDATE webhook_logins SET started = 1 WHERE state = ? AND started = 0 AND created_at > ?",
  )
    .bind(state, Date.now() - WEBHOOK_TTL_MS)
    .run();
  if (started.meta.changes !== 1) return expired;
  const authorize = new URL("https://discord.com/oauth2/authorize");
  authorize.searchParams.set("client_id", env.DISCORD_CLIENT_ID);
  authorize.searchParams.set("response_type", "code");
  authorize.searchParams.set("redirect_uri", `${url.origin}/webhook/callback`);
  authorize.searchParams.set("scope", WEBHOOK_SCOPE);
  authorize.searchParams.set("state", state);
  return new Response(null, {
    status: 302,
    headers: {
      Location: authorize.toString(),
      "Set-Cookie": `${WEBHOOK_COOKIE}=${withState(statesIn(request.headers.get("Cookie"), WEBHOOK_COOKIE), state)}; Path=/webhook; Max-Age=600; HttpOnly; Secure; SameSite=Lax`,
    },
  });
}

async function webhookCallback(request: Request, url: URL, env: Env): Promise<Response> {
  const state = url.searchParams.get("state") ?? "";
  const code = url.searchParams.get("code");
  if (!code) return page("Cancelled", "No channel was connected. You can close this tab.");
  if (!statesIn(request.headers.get("Cookie"), WEBHOOK_COOKIE).some((s) => sameText(s, state))) {
    return page("Link expired", "Press Connect channel in ClippiBoy again, in this browser.");
  }
  const row = await env.DB.prepare("SELECT key, created_at FROM webhook_logins WHERE state = ? AND started = 1")
    .bind(state)
    .first<{ key: string; created_at: number }>();
  // Spent on the first try, whatever comes of it.
  if (row) await env.DB.prepare("DELETE FROM webhook_logins WHERE state = ?").bind(state).run();
  if (!row || !fresh(row.created_at, Date.now())) {
    return page("Link expired", "Press Connect channel in ClippiBoy again.");
  }

  const tokenResponse = await fetch("https://discord.com/api/oauth2/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: `${url.origin}/webhook/callback`,
      client_id: env.DISCORD_CLIENT_ID,
      client_secret: env.DISCORD_CLIENT_SECRET,
    }),
  });
  if (!tokenResponse.ok) {
    // Not the body: on success it would hold the webhook.
    console.error("discord webhook token", tokenResponse.status);
    return page("Discord said no", "Discord did not hand over the channel. Try again from ClippiBoy.");
  }
  let granted: { access_token?: string };
  try {
    granted = (await tokenResponse.json()) as { access_token?: string };
  } catch {
    return page("Discord said no", "Discord did not hand over the channel. Try again from ClippiBoy.");
  }
  // The server's name, for "Post to <server>". Without it the channel still works.
  let guilds: unknown = null;
  if (granted.access_token) {
    try {
      const guildsResponse = await fetch("https://discord.com/api/users/@me/guilds", {
        headers: { Authorization: `Bearer ${granted.access_token}` },
      });
      if (guildsResponse.ok) guilds = await guildsResponse.json();
    } catch (err) {
      console.error("discord guilds", err);
    }
    // Not needed past this point, and not kept: revoked at once.
    await revokeToken(env, granted.access_token);
  }
  const webhook = grantedWebhook(granted, guilds);
  if (!webhook) return page("Discord said no", "Discord did not hand over the channel. Try again from ClippiBoy.");
  const back = webhookLink(await seal(row.key, webhook));
  // No cache keeps it (page): its link carries the sealed webhook.
  return page("Almost done", `Return to ClippiBoy to finish connecting ${webhook.guild}. You can close this tab.`, back, [
    `${WEBHOOK_COOKIE}=; Path=/webhook; Max-Age=0; HttpOnly; Secure; SameSite=Lax`,
  ]);
}

/**
 * The small pages of sign-in and channel connection. Only their own style and
 * script run (by nonce), nobody may frame them, and no cache keeps them: the
 * one after sign-in carries a one-time code in its link.
 */
/** A button that posts `code` back to this server — the only form a page has. */
interface PageForm {
  action: string;
  code: string;
  label: string;
}

function page(title: string, text: string, redirect?: string, cookies: string[] = [], form?: PageForm): Response {
  const nonce = randomToken(16);
  const link = redirect
    ? `<p><a href="${escapeHtml(redirect)}">Open ClippiBoy</a></p><script nonce="${nonce}">location.href=${JSON.stringify(redirect)}</script>`
    : form
      ? `<form method="post" action="${escapeHtml(form.action)}"><input type="hidden" name="code" value="${escapeHtml(form.code)}"><button type="submit">${escapeHtml(form.label)}</button></form>`
      : "";
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ClippiBoy</title>
<style nonce="${nonce}">body{margin:0;min-height:100vh;display:grid;place-items:center;background:#08080a;color:#f4f4f5;font:16px/1.5 system-ui,sans-serif}
main{background:#121215;border:1px solid #26262c;border-radius:20px;padding:32px 36px;max-width:360px;text-align:center}
h1{font-size:20px;margin:0 0 8px}p{color:#a1a1aa;margin:0}a,button{display:inline-block;margin-top:20px;background:#fff;color:#08080a;border-radius:999px;padding:10px 20px;text-decoration:none;font-weight:600}
button{border:0;font:inherit;font-weight:600;cursor:pointer;background:#f87171}</style></head>
<body><main><h1>${escapeHtml(title)}</h1><p>${escapeHtml(text)}</p>${link}</main></body></html>`;
  const headers = new Headers({
    "Content-Type": "text/html; charset=utf-8",
    "Cache-Control": "no-store",
    "Content-Security-Policy": pageCsp(nonce, form !== undefined),
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "no-referrer",
  });
  for (const cookie of cookies) headers.append("Set-Cookie", cookie);
  return new Response(html, { headers });
}

// --- Friends ------------------------------------------------------------------

async function loadMe(env: Env, me: string) {
  const row = await userById(env.DB, me);
  if (!row) throw new HttpError(401, "This account no longer exists.");
  return toMe(row);
}

interface FriendRow {
  id: string;
  username: string;
  display_name: string;
  avatar: string | null;
  status: "pending" | "accepted";
  requester: string;
  created_at: number;
}

async function listFriends(env: Env, me: string) {
  const { results } = await env.DB.prepare(
    `SELECT u.id, u.username, u.display_name, u.avatar, u.friend_code, u.allow_requests, f.status, f.requester, f.created_at
       FROM friendships f
       JOIN users u ON u.id = CASE WHEN f.user_a = ?1 THEN f.user_b ELSE f.user_a END
      WHERE f.user_a = ?1 OR f.user_b = ?1
      ORDER BY u.display_name COLLATE NOCASE`,
  )
    .bind(me)
    .all<FriendRow & { friend_code: string; allow_requests: number }>();
  const friends: User[] = [];
  const incoming: (User & { since: number })[] = [];
  const outgoing: (User & { since: number })[] = [];
  for (const row of results) {
    const user = toUser(row);
    if (row.status === "accepted") friends.push(user);
    else if (row.requester === me) outgoing.push({ ...user, since: row.created_at });
    else incoming.push({ ...user, since: row.created_at });
  }
  const blocked = await env.DB.prepare(
    `SELECT u.* FROM blocks b JOIN users u ON u.id = b.blocked WHERE b.blocker = ? ORDER BY u.display_name COLLATE NOCASE`,
  )
    .bind(me)
    .all<Parameters<typeof toUser>[0]>();
  return { friends, incoming, outgoing, blocked: blocked.results.map(toUser) };
}

/**
 * The users' apps refetch their lists — and, for "presence", get a fresh
 * snapshot of their friends' presence too (`noticeFor`). Null tells nobody.
 */
async function notify(env: Env, notice: "lists" | "presence" | null, ...users: string[]): Promise<void> {
  if (!notice) return;
  await Promise.all(
    users.map((id) =>
      env.HUB.getByName(id)
        .friendsChanged(notice === "presence")
        .catch((err) => console.error("friendsChanged", id, err)),
    ),
  );
}

/**
 * Before a pair's row is deleted, in the same batch: rests the pair
 * (`REQUEST_COOLDOWN_MS`) when the row is a request — `coolsDown` in SQL.
 * Read from the row itself, so it is never written for a row that is not there.
 */
function coolDownStatement(env: Env, a: string, b: string, now: number): D1PreparedStatement {
  return env.DB.prepare(
    `INSERT INTO request_cooldowns (user_a, user_b, ends_at)
     SELECT user_a, user_b, ?3 FROM friendships WHERE user_a = ?1 AND user_b = ?2 AND status = 'pending'
     ON CONFLICT (user_a, user_b) DO UPDATE SET ends_at = excluded.ends_at`,
  ).bind(a, b, now + REQUEST_COOLDOWN_MS);
}

async function isBlocked(env: Env, blocker: string, blocked: string): Promise<boolean> {
  const row = await env.DB.prepare("SELECT 1 FROM blocks WHERE blocker = ? AND blocked = ?")
    .bind(blocker, blocked)
    .first();
  return row !== null;
}

async function sendRequest(env: Env, me: string, query: string) {
  const text = query.trim().replace(/^@/, "");
  if (!text) throw new HttpError(400, "Enter a Discord name or a friend code.");
  const code = normalizeCode(text);
  const byCode = code ? await userByCode(env.DB, code) : null;
  const target = byCode ?? (await userByName(env.DB, text.toLowerCase()));
  if (!target) throw new HttpError(404, NOBODY);
  const outcome = await requestTo(env, me, target, byCode !== null);
  if (outcome.kind === "refuse") throw new HttpError(outcome.status, outcome.message);
  return { status: outcome.kind === "accept" ? ("accepted" as const) : ("sent" as const), user: toUser(target) };
}

/**
 * Asks `target` to be friends, or says yes if they asked first — whatever
 * `requestRule` decides. Writes and tells both sides; a refusal writes nothing.
 */
async function requestTo(
  env: Env,
  me: string,
  target: NonNullable<Awaited<ReturnType<typeof userById>>>,
  handedOut: boolean,
): Promise<RequestOutcome> {
  const [a, b] = pair(me, target.id);
  const [blockedByTarget, blockedByMe, existing, waiting, cooldown] = await Promise.all([
    isBlocked(env, target.id, me),
    isBlocked(env, me, target.id),
    env.DB.prepare("SELECT status, requester FROM friendships WHERE user_a = ? AND user_b = ?")
      .bind(a, b)
      .first<{ status: "pending" | "accepted"; requester: string }>(),
    env.DB.prepare(
      "SELECT COUNT(*) AS n FROM friendships WHERE (user_a = ?1 OR user_b = ?1) AND requester = ?1 AND status = 'pending'",
    )
      .bind(me)
      .first<{ n: number }>(),
    env.DB.prepare("SELECT ends_at FROM request_cooldowns WHERE user_a = ? AND user_b = ?")
      .bind(a, b)
      .first<{ ends_at: number }>(),
  ]);
  const now = Date.now();
  const outcome = requestRule({
    me,
    target: { id: target.id, allowRequests: target.allow_requests === 1 },
    blockedByTarget,
    blockedByMe,
    existing,
    handedOut,
    pendingOut: waiting?.n ?? 0,
    cooldownUntil: cooldown?.ends_at ?? null,
    now,
  });
  // A rest that is over goes as soon as it is seen, rather than with the
  // hourly cron. Best effort: the cron takes what this leaves.
  if (cooldown && cooldown.ends_at <= now) {
    await env.DB.prepare("DELETE FROM request_cooldowns WHERE user_a = ? AND user_b = ? AND ends_at <= ?")
      .bind(a, b, now)
      .run()
      .catch((err) => console.error("request cooldown", a, b, err));
  }
  // Only a request that writes counts against the friend limit: a refusal
  // writes nothing, and a few mistyped names must not lock the field.
  if (outcome.kind !== "refuse") await spend(env, me, "friend");
  if (outcome.kind === "accept") {
    await env.DB.prepare("UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ?").bind(a, b).run();
  } else if (outcome.kind === "send") {
    // A row that appeared meanwhile stays as it is.
    await env.DB.prepare(
      "INSERT OR IGNORE INTO friendships (user_a, user_b, requester, status, created_at) VALUES (?, ?, ?, 'pending', ?)",
    )
      .bind(a, b, me, Date.now())
      .run();
  }
  // A request sent tells both sides' lists; a yes makes them friends, which
  // their presence follows.
  if (outcome.kind === "accept") await notify(env, noticeFor("pending", "accepted"), me, target.id);
  else if (outcome.kind === "send") await notify(env, noticeFor(null, "pending"), me, target.id);
  return outcome;
}

async function acceptRequest(env: Env, me: string, other: string) {
  const [a, b] = pair(me, other);
  const result = await env.DB.prepare(
    "UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ? AND status = 'pending' AND requester = ?",
  )
    .bind(a, b, other)
    .run();
  if (result.meta.changes !== 1) throw new HttpError(404, "That request is gone.");
  await notify(env, noticeFor("pending", "accepted"), me, other);
  return { ok: true };
}

/**
 * Declines, withdraws or unfriends — whatever the pair had, it is gone. A
 * request withdrawn or declined rests the pair (`coolsDown`); a friendship
 * ended changes who sees whose presence, a request only the lists.
 */
async function removeFriend(env: Env, me: string, other: string) {
  const [a, b] = pair(me, other);
  const [, removed] = await env.DB.batch<{ status: PairStatus }>([
    coolDownStatement(env, a, b, Date.now()),
    env.DB.prepare("DELETE FROM friendships WHERE user_a = ? AND user_b = ? RETURNING status").bind(a, b),
  ]);
  await notify(env, noticeFor(removed.results[0]?.status ?? null, null), me, other);
  return { ok: true };
}

async function blockUser(env: Env, me: string, other: string) {
  if (other === me) throw new HttpError(400, "That's you.");
  if (!(await userById(env.DB, other))) throw new HttpError(404, "Not found.");
  const [a, b] = pair(me, other);
  const now = Date.now();
  const [, unfriended, blocked] = await env.DB.batch<{ status: PairStatus }>([
    // A request blocked is a request declined: the pair rests (`coolsDown`).
    coolDownStatement(env, a, b, now),
    env.DB.prepare("DELETE FROM friendships WHERE user_a = ? AND user_b = ? RETURNING status").bind(a, b),
    env.DB.prepare("INSERT OR IGNORE INTO blocks (blocker, blocked, created_at) VALUES (?, ?, ?)").bind(me, other, now),
  ]);
  // The other side learns only of a row that went; a repeat tells nobody.
  const removed = unfriended.results[0]?.status ?? null;
  if (removed) await notify(env, noticeFor(removed, null), me, other);
  else if (blocked.meta.changes > 0) await notify(env, "lists", me);
  return { ok: true };
}

/** The fingerprint a deleted account leaves behind, or null when the secret is missing. */
async function departedHash(env: Env, discordId: string): Promise<string | null> {
  const hash = await fingerprint(env.DEPARTED_PEPPER, discordId);
  // Accounts still come and go without it; only the week's limit and a
  // suspension no longer outlive a deletion.
  if (hash === null) console.error("DEPARTED_PEPPER is not set: deleted accounts leave nothing behind");
  return hash;
}

async function deleteAccount(env: Env, me: string): Promise<Response> {
  const others = await related(env.DB, me);
  const row = await env.DB.prepare("SELECT discord_id FROM users WHERE id = ?").bind(me).first<{ discord_id: string }>();
  const departed = row ? await departedHash(env, row.discord_id) : null;
  // The connections and the presence first, while a failure can still be
  // retried: after the rows below are gone a retry is a 401, and presence and
  // "last seen" stayed in the hub for good. Friends are still friends here, so
  // they hear the user went offline. Not yet with the 4001 that signs the app
  // out: should anything below fail, the app is still signed in to an account
  // that is still there, and dials back in. Once the account is gone that dial
  // is a 401, and an app that got in before it hears the 4001 below.
  await env.HUB.getByName(me).closeAll("account being deleted", me, CLOSE_DELETING);
  await deleteSharesOf(env, me);
  // Cascades to sessions, friendships, blocks and shares. In one batch with
  // what outlives it, read at that moment: a suspension set a second earlier
  // still goes along.
  const remove = env.DB.prepare("DELETE FROM users WHERE id = ?").bind(me);
  await env.DB.batch(departed ? [departStatement(env.DB, me, departed, Date.now()), remove] : [remove]);
  // Friends lose a friend, and with it a presence; those with a request only a list entry.
  await Promise.all([
    notify(env, noticeFor("accepted", null), ...others.filter((row) => row.status === "accepted").map((row) => row.id)),
    notify(env, noticeFor("pending", null), ...others.filter((row) => row.status === "pending").map((row) => row.id)),
  ]);
  // Whatever an app that got in again meanwhile left, now with the 4001. The
  // account is gone either way, so this must not turn into an error.
  await env.HUB.getByName(me)
    .closeAll()
    .catch((err) => console.error("account deleted, hub", me, err));
  return json({ ok: true });
}
