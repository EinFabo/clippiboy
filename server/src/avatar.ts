// Avatars on public pages. The avatar link kept for an account is Discord's,
// and it carries the Discord id (cdn.discordapp.com/avatars/<id>/…). A share
// page and the page after its "Add as friend" show the uploader's picture
// through our own route instead, keyed by the share: /c/<share id>/avatar.
// The server fetches Discord's picture itself, and the page never names it.
//
// No imports, so `node --test` can load this file as it is.

/** A 128 px avatar is a few KB; anything far past that is not one. */
export const MAX_AVATAR_BYTES = 256 * 1024;

/** The public route for the uploader's picture of a share. */
export function avatarRoute(origin: string, shareId: string): string {
  return `${origin}/c/${shareId}/avatar`;
}

/**
 * The Discord picture behind a stored avatar link, or null when the link is
 * not one we make (`avatarUrl` in db.ts): a custom avatar or one of Discord's
 * defaults, as PNG, on Discord's CDN. Nothing else is ever fetched.
 */
export function discordAvatar(stored: string | null): string | null {
  if (!stored) return null;
  let url: URL;
  try {
    url = new URL(stored);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.hostname !== "cdn.discordapp.com" || url.port !== "") return null;
  const custom = /^\/avatars\/\d{1,20}\/(a_)?[0-9a-f]{32}\.png$/.test(url.pathname);
  const fallback = /^\/embed\/avatars\/\d\.png$/.test(url.pathname);
  if (!custom && !fallback) return null;
  return `https://cdn.discordapp.com${url.pathname}?size=128`;
}

/** The first bytes of every PNG. */
export function looksLikePng(head: Uint8Array): boolean {
  const magic = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  return head.length >= magic.length && magic.every((byte, i) => head[i] === byte);
}
