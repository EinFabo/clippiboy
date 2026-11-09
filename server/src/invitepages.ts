// The pages of an invite: clippiboy.com/i/<code> (someone's own invite link)
// and the "Add <name> as friend" click, from there or from a share page
// (clippiboy.com/c/<id>/add). The click leaves the invite cookie for the whole
// domain; the sign-in callback on api.clippiboy.com reads it (index.ts). The
// site proxy forwards all of these paths here.

import { userByCode } from "./db";
import { inviteCookie, ownClick, signInvite } from "./invite";
import { PAGE_ORIGIN, shareUploader } from "./links";
import { renderInvite, type InviteData } from "./page";
import { normalizeCode } from "./util";

/**
 * The key invites are signed with. `INVITE_SECRET` when it is set; until then
 * the Discord client secret, which every deploy has — the label in invite.ts
 * keeps the two uses apart. Setting the secret later only voids the invites
 * still waiting.
 */
export function inviteSecret(env: Env): string {
  return env.INVITE_SECRET || env.DISCORD_CLIENT_SECRET;
}

interface Inviter {
  id: string;
  display_name: string;
  avatar: string | null;
  friend_code: string;
  allow_requests: number;
}

function html(data: Omit<InviteData, "site">, status = 200, cookie?: string): Response {
  const headers = new Headers({
    "Content-Type": "text/html; charset=utf-8",
    "Cache-Control": "no-store",
    // A button that leaves a cookie must not be clicked through someone else's frame.
    "Content-Security-Policy": "frame-ancestors 'none'",
  });
  if (cookie) headers.set("Set-Cookie", cookie);
  return new Response(renderInvite({ site: PAGE_ORIGIN, ...data }), { status, headers });
}

function back(to: string): Response {
  return new Response(null, { status: 303, headers: { Location: to, "Cache-Control": "no-store" } });
}

/** Clicked on our own page: the cookie, and what to do next. */
async function added(request: Request, env: Env, inviter: Inviter, from: string): Promise<Response> {
  if (inviter.allow_requests !== 1 || !ownClick(request.headers.get("Sec-Fetch-Site"), request.headers.get("Referer"), PAGE_ORIGIN)) {
    return back(from);
  }
  const token = await signInvite(inviteSecret(env), inviter.id, Date.now());
  return html(
    {
      inviter: { name: inviter.display_name, avatar: inviter.avatar },
      add: null,
      added: true,
      open: `clippiboy://add?code=${inviter.friend_code}`,
    },
    200,
    inviteCookie(token),
  );
}

/** clippiboy.com/i/<code>, and /i/<code>/add behind its button. */
export async function invitePage(request: Request, env: Env, rawCode: string, add: boolean): Promise<Response> {
  const code = normalizeCode(rawCode);
  const row = code ? await userByCode(env.DB, code) : null;
  // A suspended account invites nobody.
  const banned = row ? (row as { banned_at?: number | null }).banned_at != null : false;
  if (!row || banned) return html({ inviter: null, add: null, added: false, open: null }, 404);
  const self = `${PAGE_ORIGIN}/i/${row.friend_code}`;
  if (add) return added(request, env, row, self);
  return html({
    inviter: { name: row.display_name, avatar: row.avatar },
    add: row.allow_requests === 1 ? `${self}/add` : null,
    added: false,
    open: null,
  });
}

/** clippiboy.com/c/<id>/add — the share page's button; back to the clip if it no longer applies. */
export async function shareInvite(request: Request, env: Env, id: string): Promise<Response> {
  const uploader = await shareUploader(env, id);
  const page = `${PAGE_ORIGIN}/c/${id}`;
  return uploader ? added(request, env, uploader, page) : back(page);
}
