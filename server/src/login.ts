// Sign-in bookkeeping that needs no Worker runtime.

/** How many sign-in starts one browser may have waiting at once. */
export const STATES_KEPT = 3;

/** From the start of a sign-in to the app's exchange of its code. */
export const LOGIN_TTL_MS = 10 * 60 * 1000;
/** How far ahead of this server's clock a state may claim to be from (another server's clock). */
const CLOCK_SLACK_MS = 60 * 1000;

/** The hash of the app's verifier, as `/auth/start` takes it: base64url of 32 bytes. */
export const CHALLENGE_PATTERN = /^[A-Za-z0-9_-]{43}$/;
/**
 * What a suspended account's sign-in is stored with in place of the app's
 * challenge. Its one-time code is on the suspended page, for "Delete account"
 * (`/auth/delete`) alone: no verifier hashes to this, and `/auth/exchange`
 * takes only a code stored with a challenge (`signsIn`).
 */
export const DELETE_ONLY = "";

/** Whether a sign-in stored with this challenge may be traded for a session. */
export function signsIn(challenge: string): boolean {
  return CHALLENGE_PATTERN.test(challenge);
}
/** Domain separation: the key is the Discord client secret, which has other uses. */
const STATE_LABEL = "clippiboy-login-state:v1:";
/** base64url of six bytes of start time (ms) and 18 random ones. */
const HEAD_LENGTH = 32;
const STATE_PATTERN = /^[A-Za-z0-9_-]{118}$/;

/** Compares without leaking where the first difference sits. */
export function sameText(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

/** The states the browser's cookie holds, oldest first. */
export function statesIn(cookieHeader: string | null, name: string): string[] {
  for (const part of (cookieHeader ?? "").split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=").split(".").filter(Boolean);
  }
  return [];
}

/** The cookie value after another start: newest last, at most STATES_KEPT (base64url has no `.`). */
export function withState(states: string[], state: string): string {
  return [...states, state].slice(-STATES_KEPT).join(".");
}

/** The cookie value once `state` is spent: the browser's other starts stay. */
export function withoutState(states: string[], state: string): string {
  return states.filter((s) => !sameText(s, state)).join(".");
}

/**
 * The state a sign-in start sends to Discord and keeps in the browser's
 * cookie: the start time, a few random bytes, the app's challenge and an
 * HMAC-SHA256 over them, all base64url — so the start writes nothing, and the
 * callback learns challenge and age from the state alone. Nobody without the
 * key can make one, or change the challenge in one. `random` is for tests.
 */
export async function signState(
  secret: string,
  challenge: string,
  now: number,
  random: Uint8Array = crypto.getRandomValues(new Uint8Array(18)),
): Promise<string> {
  // An empty key would let anybody make states.
  if (!secret) throw new Error("no key to sign sign-in states with");
  const head = new Uint8Array(24);
  const high = Math.floor(now / 2 ** 32);
  const low = now % 2 ** 32;
  head.set([(high >> 8) & 255, high & 255, (low >>> 24) & 255, (low >> 16) & 255, (low >> 8) & 255, low & 255]);
  head.set(random.subarray(0, 18), 6);
  const body = base64url(head) + challenge;
  return body + (await mac(secret, body));
}

/**
 * The challenge and start time a state carries, or null for one that is
 * forged, changed, of another key, older than LOGIN_TTL_MS or not a state at
 * all (a random one from before, say) — and for every one without a key. The MAC is compared as text in
 * constant time, so a state has one spelling only.
 */
export async function verifyState(
  secret: string,
  state: string,
  now: number,
): Promise<{ challenge: string; issuedAt: number } | null> {
  if (!secret || !STATE_PATTERN.test(state)) return null;
  const body = state.slice(0, HEAD_LENGTH + 43);
  if (!sameText(await mac(secret, body), state.slice(HEAD_LENGTH + 43))) return null;
  const head = fromBase64url(state.slice(0, HEAD_LENGTH));
  if (!head || head.length !== 24) return null;
  const issuedAt = ((head[0] << 8) | head[1]) * 2 ** 32 + ((head[2] << 24) >>> 0) + (head[3] << 16) + (head[4] << 8) + head[5];
  if (now - issuedAt > LOGIN_TTL_MS || issuedAt > now + CLOCK_SLACK_MS) return null;
  return { challenge: state.slice(HEAD_LENGTH, HEAD_LENGTH + 43), issuedAt };
}

async function mac(secret: string, body: string): Promise<string> {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  return base64url(new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(STATE_LABEL + body))));
}

function base64url(data: Uint8Array): string {
  let text = "";
  for (const byte of data) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64url(text: string): Uint8Array | null {
  try {
    const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
    return Uint8Array.from(binary, (c) => c.charCodeAt(0));
  } catch {
    return null;
  }
}

/**
 * The policy of the sign-in and channel pages: nothing loads, only the page's
 * own style and script (by `nonce`) run, no form goes anywhere — or, with
 * `forms`, only back to this server (the suspended page's Delete account) —
 * and nobody frames them.
 */
export function pageCsp(nonce: string, forms = false): string {
  return [
    "default-src 'none'",
    `style-src 'nonce-${nonce}'`,
    `script-src 'nonce-${nonce}'`,
    "base-uri 'none'",
    forms ? "form-action 'self'" : "form-action 'none'",
    "frame-ancestors 'none'",
  ].join("; ");
}
