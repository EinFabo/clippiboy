// What the site proxy checks and adds, without anything of the Worker in it —
// imports nothing, so `node --test` loads it as it is (src/siteproxy.test.ts).

/** Where GitHub Pages serves the site. */
export const ORIGIN = "https://einfabo.github.io/clippiboy";
export const HOME = "https://clippiboy.com";
/** Installers are fetched from here and nowhere else. */
export const DOWNLOADS = "https://github.com/EinFabo/clippiboy/releases/download/";

/**
 * Downloads counted in a day, all links together, unless the `DOWNLOADS_PER_DAY`
 * var says otherwise. The per-address limit keeps one address from filling
 * the counts; this keeps many together from spending the database's daily
 * writes through them. Past it the setup is still served, only not counted.
 */
export const DOWNLOADS_PER_DAY = 2000;

/** The day's ceiling: the var when it is a whole number above zero, else the default. */
export function downloadCeiling(configured: string | undefined): number {
  const value = Number(configured);
  return Number.isInteger(value) && value > 0 ? value : DOWNLOADS_PER_DAY;
}

/**
 * The release the page's release.json names, or null when it is not one of
 * ours: the setup must be a download of this repository's releases, and the
 * version plain numbers, since it goes into the file name of the answer.
 */
export function validRelease(raw: unknown): { version: string; url: string } | null {
  if (typeof raw !== "object" || raw === null) return null;
  const { version, setup } = raw as { version?: unknown; setup?: unknown };
  if (typeof version !== "string" || !/^\d{1,4}\.\d{1,4}\.\d{1,4}$/.test(version)) return null;
  if (typeof setup !== "object" || setup === null) return null;
  const { url } = setup as { url?: unknown };
  if (typeof url !== "string") return null;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  // Checked after parsing, so "..", escapes or a user part cannot lead elsewhere.
  if (parsed.username || parsed.password || !parsed.href.startsWith(DOWNLOADS)) return null;
  return { version, url: parsed.href };
}

/**
 * A redirect from GitHub Pages, pointed back at clippiboy.com. Only a path of
 * our own comes out: `//host` or `/\host` would send the browser to another
 * site, so leading slashes are folded into one.
 */
export function rewriteLocation(location: string): string {
  let rest: string;
  if (location.startsWith(ORIGIN)) rest = location.slice(ORIGIN.length);
  else if (location.startsWith("/clippiboy")) rest = location.slice("/clippiboy".length);
  else return location;
  // "/clippiboyfoo" is not under the site: left as it is.
  if (rest !== "" && !/^[/\\?#]/.test(rest)) return location;
  const path = "/" + rest.replace(/^[/\\]+/, "");
  return location.startsWith(ORIGIN) ? HOME + path : path;
}

/**
 * The site's own policy. Everything it loads is its own (main.js, style.css,
 * the font, the pictures); main.js reads /stats.json and release.json here and
 * only falls back to the GitHub API without the latter. No inline script or
 * style in the pages, no forms, and nobody frames them.
 */
export const SITE_CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self' data:",
  "font-src 'self'",
  "connect-src 'self' https://api.github.com",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
].join("; ");

/** Adds the site's security headers to a page passed through from GitHub Pages. */
export function withSecurityHeaders(headers: Headers): Headers {
  headers.set("Content-Security-Policy", SITE_CSP);
  headers.set("X-Content-Type-Options", "nosniff");
  headers.set("Referrer-Policy", "strict-origin-when-cross-origin");
  return headers;
}
