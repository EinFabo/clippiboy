// The pages of an invite: clippiboy.com/i/<code> (someone's own invite link)
// and the "Add <name> as friend" click, from there or from a share page
// (clippiboy.com/c/<id>/add). The click leaves the invite cookie for the whole
// domain; the sign-in callback on api.clippiboy.com reads it (index.ts). The
// site proxy forwards all of these paths here.

import { userByCode } from "./db";
import { inviteCookie, inviteKey, inviteOpen, ownClick, signInvite } from "./invite";
import { PAGE_ORIGIN, shareUploader } from "./links";
import { renderInvite, type InviteData } from "./page";
import { normalizeCode } from "./util";

/** The key invites are signed with, or null — then there are no invites (invite.ts). */
export function inviteSecret(env: Env): string | null {
  return inviteKey(env.INVITE_SECRET, env.DISCORD_CLIENT_SECRET);
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

const INVALID: Omit<InviteData, "site"> = { state: "invalid", inviter: null, add: null, open: null };

/**
 * Clicked on our own page: the cookie, and what to do next. From a share page
 * the next page names the uploader (the share page did already) but offers no
 * "Open ClippiBoy": its link would carry the friend code, which never changes,
 * to anyone who opens a public share link.
 */
async function added(request: Request, env: Env, inviter: Inviter, from: string, fromShare: boolean): Promise<Response> {
  const secret = inviteSecret(env);
  if (
    !secret ||
    inviter.allow_requests !== 1 ||
    !ownClick(request.url, request.headers.get("Sec-Fetch-Site"), request.headers.get("Referer"), PAGE_ORIGIN)
  ) {
    return back(from);
  }
  const token = await signInvite(secret, inviter.id, Date.now());
  return html(
    {
      state: "added",
      inviter: fromShare ? { name: inviter.display_name, avatar: inviter.avatar } : null,
      add: null,
      open: fromShare ? null : `clippiboy://add?code=${inviter.friend_code}`,
    },
    200,
    inviteCookie(token),
  );
}

/**
 * clippiboy.com/i/<code>, and /i/<code>/add behind its button. Someone who
 * takes no requests, or is suspended, has no invite link: the same page as
 * for a code nobody has.
 */
export async function invitePage(request: Request, env: Env, rawCode: string, add: boolean): Promise<Response> {
  const code = normalizeCode(rawCode);
  const row = code ? await userByCode(env.DB, code) : null;
  const user = row ? { allowRequests: row.allow_requests === 1, banned: row.banned_at != null } : null;
  if (!row || !inviteOpen(user, inviteSecret(env) !== null)) return html(INVALID, 404);
  const self = `${PAGE_ORIGIN}/i/${row.friend_code}`;
  if (add) return added(request, env, row, self, false);
  return html({ state: "invite", inviter: null, add: `${self}/add`, open: null });
}

/** clippiboy.com/c/<id>/add — the share page's button; back to the clip if it no longer applies. */
export async function shareInvite(request: Request, env: Env, id: string): Promise<Response> {
  const uploader = await shareUploader(env, id);
  const page = `${PAGE_ORIGIN}/c/${id}`;
  return uploader ? added(request, env, uploader, page, true) : back(page);
}
