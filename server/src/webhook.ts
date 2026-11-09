// "Post clips to a Discord channel": Discord's `webhook.incoming` scope lets
// the user pick a server and a channel in Discord's own dialog, and the token
// response carries a webhook for that channel. The webhook is a credential —
// whoever holds its URL can post there — so the server never keeps it. The app
// starts the flow with a key of its own (POST /webhook/begin, signed in); the
// callback seals the webhook with that key and hands it over once in the
// clippiboy:// link. A program that grabs the link gets ciphertext, and the
// database only ever held the key, for ten minutes.
//
// Imports nothing, so it can be tested under plain Node (`npm test`).

export const WEBHOOK_COOKIE = "cb_webhook";
/** From the app's start to Discord's answer — like a sign-in. */
export const WEBHOOK_TTL_MS = 10 * 60 * 1000;
/** The webhook, and the server names to say which server it posts to. */
export const WEBHOOK_SCOPE = "webhook.incoming guilds";
/** Shown when the server's name could not be read. */
export const UNKNOWN_GUILD = "your server";
/** Binds a sealed webhook to this purpose (AES-GCM additional data). */
const SEAL_LABEL = "clippiboy-webhook:v1";
/** Discord's own limit for a server name. */
const GUILD_NAME_MAX = 100;

/** The webhook as the app gets it. The URL is built from id and token. */
export interface Granted {
  id: string;
  token: string;
  guildId: string;
  channelId: string;
  guild: string;
}

/** An AES-256 key as the app sends it: 32 bytes, base64url without padding. */
export function isKey(text: unknown): text is string {
  return typeof text === "string" && /^[A-Za-z0-9_-]{43}$/.test(text);
}

/** A state as `randomToken(24)` makes it. */
export function isState(text: unknown): text is string {
  return typeof text === "string" && /^[A-Za-z0-9_-]{32}$/.test(text);
}

/** Started recently enough to still be finished? */
export function fresh(createdAt: number, now: number): boolean {
  return now - createdAt <= WEBHOOK_TTL_MS && createdAt <= now + 60_000;
}

const SNOWFLAKE = /^[0-9]{1,20}$/;
const TOKEN = /^[A-Za-z0-9_-]{1,200}$/;

/**
 * The webhook out of Discord's token response, with the server's name from
 * `/users/@me/guilds` — or null when the response holds no usable webhook.
 * Only the shapes Discord hands out pass: id and token go into a URL path.
 */
export function grantedWebhook(tokenResponse: unknown, guilds: unknown): Granted | null {
  const webhook = (tokenResponse as { webhook?: Record<string, unknown> } | null)?.webhook;
  if (!webhook || typeof webhook !== "object") return null;
  const { id, token, guild_id: guildId, channel_id: channelId } = webhook;
  if (typeof id !== "string" || !SNOWFLAKE.test(id)) return null;
  if (typeof token !== "string" || !TOKEN.test(token)) return null;
  if (typeof guildId !== "string" || !SNOWFLAKE.test(guildId)) return null;
  if (typeof channelId !== "string" || !SNOWFLAKE.test(channelId)) return null;
  return { id, token, guildId, channelId, guild: guildName(guilds, guildId) };
}

/** The server's name from a `/users/@me/guilds` answer, tidied; a stand-in if it is not there. */
export function guildName(guilds: unknown, guildId: string): string {
  if (!Array.isArray(guilds)) return UNKNOWN_GUILD;
  const found = guilds.find((g) => g && typeof g === "object" && (g as { id?: unknown }).id === guildId) as
    | { name?: unknown }
    | undefined;
  if (typeof found?.name !== "string") return UNKNOWN_GUILD;
  // Control characters out, runs of space to one.
  const name = [...found.name.replace(/[\u0000-\u001f\u007f]/g, "").replace(/\s+/g, " ").trim()]
    .slice(0, GUILD_NAME_MAX)
    .join("");
  return name || UNKNOWN_GUILD;
}

/** The webhook sealed with the app's key: base64url of IV (12 bytes) and AES-GCM ciphertext with its tag. */
export async function seal(key: string, granted: Granted): Promise<string> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plain = new TextEncoder().encode(JSON.stringify(granted));
  const sealed = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: "AES-GCM", iv, additionalData: new TextEncoder().encode(SEAL_LABEL) },
      await aesKey(key, "encrypt"),
      plain,
    ),
  );
  const out = new Uint8Array(iv.length + sealed.length);
  out.set(iv);
  out.set(sealed, iv.length);
  return toBase64url(out);
}

/** The other way round — what the app does; here for the tests. Null for anything forged or under another key. */
export async function unseal(key: string, data: string): Promise<Granted | null> {
  try {
    const bytes = fromBase64url(data);
    if (bytes.length < 12 + 16) return null;
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: bytes.slice(0, 12), additionalData: new TextEncoder().encode(SEAL_LABEL) },
      await aesKey(key, "decrypt"),
      bytes.slice(12),
    );
    return JSON.parse(new TextDecoder().decode(plain)) as Granted;
  } catch {
    return null;
  }
}

/** The link the callback page opens. */
export function webhookLink(sealed: string): string {
  return `clippiboy://webhook?data=${sealed}`;
}

async function aesKey(key: string, use: "encrypt" | "decrypt"): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", fromBase64url(key), { name: "AES-GCM" }, false, [use]);
}

function toBase64url(data: Uint8Array): string {
  let text = "";
  for (const byte of data) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64url(text: string): Uint8Array {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}
