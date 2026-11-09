// Invites: "Add <name> as friend" on a share page or a personal invite link
// (clippiboy.com/i/<code>) leaves a signed cookie for the whole domain. Signing
// in with Discord runs through api.clippiboy.com in the same browser — even
// after installing the app first — so the callback reads the cookie and sends
// the friend request then. The one who invited only has to accept.
//
// Imports nothing, so it can be tested under plain Node (`npm test`).

const DAY_MS = 24 * 60 * 60 * 1000;

export const INVITE_COOKIE = "cb_invite";
/** Long enough to install the app and sign in, short enough to be forgotten. */
export const INVITE_TTL_MS = 14 * DAY_MS;
/** Every name under it: clippiboy.com sets it, api.clippiboy.com reads it. */
export const COOKIE_DOMAIN = ".clippiboy.com";
/** Domain separation: the key may be shared with another purpose (see `inviteKey`). */
const LABEL = "clippiboy-invite:v1:";
/** An account counts as made after the click if it is no older than this before it (clock steps). */
export const ISSUE_SLACK_MS = 60 * 1000;

/**
 * The key invites are signed with: `INVITE_SECRET` when it is set, else the
 * Discord client secret, which every deploy has — the label keeps the two uses
 * apart. Null when neither is there: then there are no invites at all, rather
 * than tokens signed with an empty key that anyone could make.
 */
export function inviteKey(inviteSecret: string | undefined, discordSecret: string | undefined): string | null {
  return inviteSecret || discordSecret || null;
}

function base64url(data: Uint8Array): string {
  let text = "";
  for (const byte of data) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64url(text: string): Uint8Array<ArrayBuffer> | null {
  if (!/^[A-Za-z0-9_-]+$/.test(text)) return null;
  let binary: string;
  try {
    binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  } catch {
    return null;
  }
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

async function hmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
    "verify",
  ]);
}

/**
 * `<user id>.<expiry in seconds, base 36>.<HMAC-SHA256>`. The id is a UUID
 * (hex and dashes), so the dot cannot occur in it.
 */
export async function signInvite(secret: string, userId: string, now: number): Promise<string> {
  const expires = Math.floor((now + INVITE_TTL_MS) / 1000).toString(36);
  const body = `${userId}.${expires}`;
  const mac = await crypto.subtle.sign("HMAC", await hmacKey(secret), new TextEncoder().encode(LABEL + body));
  return `${body}.${base64url(new Uint8Array(mac))}`;
}

export interface Invite {
  inviter: string;
  /** When the click was, ms since the epoch: the expiry less the lifetime. */
  issuedAt: number;
}

/** Who invited and when, or null for anything forged, mangled or expired. */
export async function verifyInvite(secret: string, token: string, now: number): Promise<Invite | null> {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const [userId, expires, mac] = parts;
  if (!/^[0-9a-f-]{36}$/.test(userId) || !/^[0-9a-z]{1,10}$/.test(expires)) return null;
  const signature = fromBase64url(mac);
  if (!signature) return null;
  // `verify` compares in constant time.
  const valid = await crypto.subtle.verify(
    "HMAC",
    await hmacKey(secret),
    signature,
    new TextEncoder().encode(`${LABEL}${userId}.${expires}`),
  );
  if (!valid) return null;
  const expiresAt = parseInt(expires, 36) * 1000;
  return expiresAt > now ? { inviter: userId, issuedAt: expiresAt - INVITE_TTL_MS } : null;
}

/**
 * Does someone's invite link work? Not for someone who takes no requests or
 * is suspended — their link looks like one nobody has, without a name — and
 * not at all without a key.
 */
export function inviteOpen(user: { allowRequests: boolean; banned: boolean } | null, haveKey: boolean): boolean {
  return haveKey && user !== null && user.allowRequests && !user.banned;
}

/** A cookie's value from a Cookie header, or null. */
export function cookieValue(header: string | null, name: string): string | null {
  for (const part of (header ?? "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=") || null;
  }
  return null;
}

export function inviteCookie(token: string): string {
  return `${INVITE_COOKIE}=${token}; Domain=${COOKIE_DOMAIN}; Path=/; Max-Age=${INVITE_TTL_MS / 1000}; HttpOnly; Secure; SameSite=Lax`;
}

export function clearInviteCookie(): string {
  return `${INVITE_COOKIE}=; Domain=${COOKIE_DOMAIN}; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax`;
}

/**
 * Was the click on our own page? A cookie for a request sent at the next
 * sign-in is worth something to whoever is named in it: another site that
 * navigates a visitor to `/i/<code>/add` must not plant one. Browsers say where
 * a navigation came from in `Sec-Fetch-Site`; older ones only in `Referer`.
 * Neither — refused; the visitor lands on the page and can click there. And
 * only on clippiboy.com itself (the site proxy hands the request over with its
 * URL unchanged): a same-origin click on api.clippiboy.com or workers.dev is
 * no click on our page.
 */
export function ownClick(requestUrl: string, secFetchSite: string | null, referer: string | null, origin: string): boolean {
  if (new URL(requestUrl).origin !== origin) return false;
  if (secFetchSite !== null) return secFetchSite === "same-origin";
  if (!referer) return false;
  try {
    return new URL(referer).origin === origin;
  } catch {
    return false;
  }
}

/**
 * Whom the sign-in asks: only for a valid invite, only from an account made
 * after the click — the invite is for someone new to ClippiBoy; signing in
 * again within the fortnight must not ask again after a decline, and another
 * account in the same browser must not ask in their place (those who have the
 * app use "Open ClippiBoy" instead) — and never oneself. A suspended inviter is
 * asked by nobody. What happens then (already friends, blocked, no requests
 * taken) is the ordinary request rule's business.
 */
export function inviteTarget(
  invite: Invite | null,
  me: { id: string; createdAt: number },
  inviter: { banned: boolean } | null,
): string | null {
  if (!invite || !inviter || inviter.banned) return null;
  if (invite.inviter === me.id) return null;
  return me.createdAt >= invite.issuedAt - ISSUE_SLACK_MS ? invite.inviter : null;
}
