// Where scripts/e2e.mjs sends its requests. A local `wrangler dev` unless said
// otherwise; anything that is not this computer only with `--production` — a
// run against the deployed worker writes test accounts into the real database.

export const LOCAL = "http://127.0.0.1:8799";
export const PRODUCTION = "https://api.clippiboy.com";

const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]"]);

/**
 * The base URL from `FRIENDS_SERVER` and the command line's flags, or an error
 * that says why not.
 * @param {string | undefined} wanted
 * @param {string[]} args
 * @returns {{ base: string } | { error: string }}
 */
export function e2eTarget(wanted, args) {
  const production = args.includes("--production");
  const raw = wanted || (production ? PRODUCTION : LOCAL);
  let url;
  try {
    url = new URL(raw);
  } catch {
    return { error: `FRIENDS_SERVER is not a URL: ${raw}` };
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return { error: `FRIENDS_SERVER must be http or https: ${raw}` };
  }
  if (!LOOPBACK.has(url.hostname) && !production) {
    return { error: `${url.origin} is not this computer; add --production to run against it` };
  }
  return { base: url.origin };
}
