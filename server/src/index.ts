import {
  authenticate,
  pair,
  relatedIds,
  toMe,
  toUser,
  tokenHash,
  upsertDiscordUser,
  userByCode,
  userById,
  userByName,
  type DiscordUser,
  type User,
} from "./db";
import { base64url, body, escapeHtml, HttpError, json, normalizeCode, randomToken, sameText, sha256 } from "./util";
import { createShare, deleteShare, deleteSharesOf, putPoster, shareFile, sharePage, shareQuota, sweep } from "./links";

export { Hub } from "./hub";

const LOGIN_TTL_MS = 10 * 60 * 1000;
const STATE_COOKIE = "cb_login";

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
    ctx.waitUntil(sweep(env));
  },
} satisfies ExportedHandler<Env>;

async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname.replace(/\/+$/, "") || "/";
  const method = request.method;

  if (path === "/" && method === "GET") return new Response("ClippiBoy friends server");
  if (path === "/auth/start" && method === "GET") return authStart(url, env);
  if (path === "/auth/callback" && method === "GET") return authCallback(request, url, env);
  if (path === "/auth/exchange" && method === "POST") return authExchange(request, env);

  // Share links are for anyone who has the link.
  const shared = path.match(/^\/c\/([a-z0-9]+)(?:\.(mp4|jpg))?$/);
  if (shared && (method === "GET" || method === "HEAD")) {
    const [, id, kind] = shared;
    return kind ? shareFile(request, env, id, kind as "mp4" | "jpg") : sharePage(env, id);
  }

  const me = await authenticate(request, env.DB);

  if (path === "/shares/quota" && method === "GET") return shareQuota(env, me);
  if (path === "/shares" && method === "POST") return createShare(request, env, me);
  const share = path.match(/^\/shares\/([a-z0-9]+)(\/poster)?$/);
  if (share) {
    const [, id, isPoster] = share;
    if (isPoster && method === "PUT") return putPoster(request, env, me, id);
    if (!isPoster && method === "DELETE") return deleteShare(env, me, id);
  }

  if (path === "/ws" && method === "GET") {
    const headers = new Headers(request.headers);
    headers.set("X-User", me);
    return env.HUB.getByName(me).fetch(new Request(request, { headers }));
  }
  if (path === "/auth/logout" && method === "POST") {
    const token = request.headers.get("Authorization")!.slice(7).trim();
    await env.DB.prepare("DELETE FROM sessions WHERE token_hash = ?").bind(await tokenHash(token)).run();
    return json({ ok: true });
  }
  if (path === "/me" && method === "GET") return json(await loadMe(env, me));
  if (path === "/me" && method === "PATCH") {
    const { allowRequests } = await body<{ allowRequests?: boolean }>(request);
    if (typeof allowRequests === "boolean") {
      await env.DB.prepare("UPDATE users SET allow_requests = ? WHERE id = ?").bind(allowRequests ? 1 : 0, me).run();
    }
    return json(await loadMe(env, me));
  }
  if (path === "/me" && method === "DELETE") return deleteAccount(env, me);
  if (path === "/friends" && method === "GET") return json(await listFriends(env, me));
  if (path === "/friends/request" && method === "POST") {
    const { query } = await body<{ query?: string }>(request);
    return json(await sendRequest(env, me, String(query ?? "")));
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

// --- Login --------------------------------------------------------------------
//
// The app opens /auth/start in the browser with the hash of a verifier it keeps.
// Discord sends the browser back to /auth/callback, which hands the app a
// one-time code through clippiboy://auth. Only the app that knows the verifier
// can trade that code for a session — another program that grabs the link
// cannot. The cookie ties the callback to the browser that started the login,
// so nobody can send someone a half-finished login of their own to complete.

async function authStart(url: URL, env: Env): Promise<Response> {
  const challenge = url.searchParams.get("challenge") ?? "";
  if (!/^[A-Za-z0-9_-]{43}$/.test(challenge)) throw new HttpError(400, "Missing challenge.");
  if (!env.DISCORD_CLIENT_ID) throw new HttpError(503, "Discord login is not configured yet.");
  const now = Date.now();
  await env.DB.prepare("DELETE FROM logins WHERE created_at < ?").bind(now - LOGIN_TTL_MS).run();
  const state = randomToken(24);
  await env.DB.prepare("INSERT INTO logins (state, challenge, created_at) VALUES (?, ?, ?)")
    .bind(state, challenge, now)
    .run();
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
      "Set-Cookie": `${STATE_COOKIE}=${state}; Path=/auth; Max-Age=600; HttpOnly; Secure; SameSite=Lax`,
    },
  });
}

async function authCallback(request: Request, url: URL, env: Env): Promise<Response> {
  const state = url.searchParams.get("state") ?? "";
  const code = url.searchParams.get("code");
  const cookie = (request.headers.get("Cookie") ?? "")
    .split(";")
    .map((part) => part.trim().split("="))
    .find(([name]) => name === STATE_COOKIE)?.[1];
  if (!code) return page("Sign-in cancelled", "You can close this tab and try again from ClippiBoy.");
  if (!cookie || !sameText(cookie, state)) {
    return page("Sign-in expired", "Start the sign-in again from ClippiBoy, in this browser.");
  }
  const login = await env.DB.prepare("SELECT created_at FROM logins WHERE state = ? AND code IS NULL")
    .bind(state)
    .first<{ created_at: number }>();
  if (!login || Date.now() - login.created_at > LOGIN_TTL_MS) {
    return page("Sign-in expired", "Start the sign-in again from ClippiBoy.");
  }

  const tokenResponse = await fetch("https://discord.com/api/oauth2/token", {
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
  if (!tokenResponse.ok) {
    console.error("discord token", tokenResponse.status, await tokenResponse.text());
    return page("Discord said no", "Discord did not confirm the sign-in. Try again from ClippiBoy.");
  }
  const { access_token } = (await tokenResponse.json()) as { access_token: string };
  const userResponse = await fetch("https://discord.com/api/users/@me", {
    headers: { Authorization: `Bearer ${access_token}` },
  });
  if (!userResponse.ok) return page("Discord said no", "Could not read your Discord profile.");
  const userId = await upsertDiscordUser(env.DB, (await userResponse.json()) as DiscordUser);

  const oneTime = randomToken(24);
  await env.DB.prepare("UPDATE logins SET code = ?, user_id = ? WHERE state = ?").bind(oneTime, userId, state).run();
  const back = `clippiboy://auth?code=${oneTime}`;
  return page(
    "Signed in",
    "Back to ClippiBoy — you can close this tab.",
    back,
  );
}

async function authExchange(request: Request, env: Env): Promise<Response> {
  const { code, verifier } = await body<{ code?: string; verifier?: string }>(request);
  if (!code || !verifier) throw new HttpError(400, "Missing code or verifier.");
  const login = await env.DB.prepare("SELECT state, challenge, user_id, created_at FROM logins WHERE code = ?")
    .bind(code)
    .first<{ state: string; challenge: string; user_id: string; created_at: number }>();
  // Spent on the first try, right or wrong.
  if (login) await env.DB.prepare("DELETE FROM logins WHERE state = ?").bind(login.state).run();
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

function page(title: string, text: string, redirect?: string): Response {
  const link = redirect
    ? `<p><a href="${escapeHtml(redirect)}">Open ClippiBoy</a></p><script>location.href=${JSON.stringify(redirect)}</script>`
    : "";
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>ClippiBoy</title>
<style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#08080a;color:#f4f4f5;font:16px/1.5 system-ui,sans-serif}
main{background:#121215;border:1px solid #26262c;border-radius:20px;padding:32px 36px;max-width:360px;text-align:center}
h1{font-size:20px;margin:0 0 8px}p{color:#a1a1aa;margin:0}a{display:inline-block;margin-top:20px;background:#fff;color:#08080a;border-radius:999px;padding:10px 20px;text-decoration:none;font-weight:600}</style></head>
<body><main><h1>${escapeHtml(title)}</h1><p>${escapeHtml(text)}</p>${link}</main></body></html>`;
  return new Response(html, {
    headers: {
      "Content-Type": "text/html; charset=utf-8",
      "Set-Cookie": `${STATE_COOKIE}=; Path=/auth; Max-Age=0; HttpOnly; Secure; SameSite=Lax`,
    },
  });
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

/** Both sides' apps refetch their lists and presence. */
async function notify(env: Env, ...users: string[]): Promise<void> {
  await Promise.all(
    users.map((id) =>
      env.HUB.getByName(id)
        .friendsChanged()
        .catch((err) => console.error("friendsChanged", id, err)),
    ),
  );
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
  const target = (code && (await userByCode(env.DB, code))) || (await userByName(env.DB, text.toLowerCase()));
  // Someone who blocked you looks like someone who does not exist.
  if (!target || (await isBlocked(env, target.id, me))) {
    throw new HttpError(404, "Nobody with that name or code uses ClippiBoy.");
  }
  if (target.id === me) throw new HttpError(400, "That's you.");
  if (await isBlocked(env, me, target.id)) throw new HttpError(409, "You blocked them — unblock them first.");

  const [a, b] = pair(me, target.id);
  const existing = await env.DB.prepare("SELECT status, requester FROM friendships WHERE user_a = ? AND user_b = ?")
    .bind(a, b)
    .first<{ status: string; requester: string }>();
  if (existing?.status === "accepted") throw new HttpError(409, "You are already friends.");
  if (existing?.requester === me) throw new HttpError(409, "Request already sent.");
  if (existing) {
    // They asked first — asking back is saying yes.
    await env.DB.prepare("UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ?").bind(a, b).run();
    await notify(env, me, target.id);
    return { status: "accepted" as const, user: toUser(target) };
  }
  if (target.allow_requests !== 1) throw new HttpError(403, "They don't take friend requests right now.");
  await env.DB.prepare(
    "INSERT INTO friendships (user_a, user_b, requester, status, created_at) VALUES (?, ?, ?, 'pending', ?)",
  )
    .bind(a, b, me, Date.now())
    .run();
  await notify(env, me, target.id);
  return { status: "sent" as const, user: toUser(target) };
}

async function acceptRequest(env: Env, me: string, other: string) {
  const [a, b] = pair(me, other);
  const result = await env.DB.prepare(
    "UPDATE friendships SET status = 'accepted' WHERE user_a = ? AND user_b = ? AND status = 'pending' AND requester = ?",
  )
    .bind(a, b, other)
    .run();
  if (result.meta.changes !== 1) throw new HttpError(404, "That request is gone.");
  await notify(env, me, other);
  return { ok: true };
}

/** Declines, withdraws or unfriends — whatever the pair had, it is gone. */
async function removeFriend(env: Env, me: string, other: string) {
  const [a, b] = pair(me, other);
  await env.DB.prepare("DELETE FROM friendships WHERE user_a = ? AND user_b = ?").bind(a, b).run();
  await notify(env, me, other);
  return { ok: true };
}

async function blockUser(env: Env, me: string, other: string) {
  if (other === me) throw new HttpError(400, "That's you.");
  if (!(await userById(env.DB, other))) throw new HttpError(404, "Not found.");
  const [a, b] = pair(me, other);
  await env.DB.batch([
    env.DB.prepare("DELETE FROM friendships WHERE user_a = ? AND user_b = ?").bind(a, b),
    env.DB.prepare("INSERT OR IGNORE INTO blocks (blocker, blocked, created_at) VALUES (?, ?, ?)").bind(
      me,
      other,
      Date.now(),
    ),
  ]);
  await notify(env, me, other);
  return { ok: true };
}

async function deleteAccount(env: Env, me: string): Promise<Response> {
  const related = await relatedIds(env.DB, me);
  await deleteSharesOf(env, me);
  // Cascades to sessions, friendships and blocks.
  await env.DB.prepare("DELETE FROM users WHERE id = ?").bind(me).run();
  await notify(env, ...related);
  await env.HUB.getByName(me).closeAll();
  return json({ ok: true });
}
