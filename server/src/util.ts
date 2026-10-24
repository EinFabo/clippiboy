import { readLimited } from "./limits";
import { randomCode } from "./shares";

export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export function json(body: unknown, status = 200): Response {
  return Response.json(body, { status });
}

/** A JSON body of at most 4 KB — more is refused unread. */
export async function body<T>(request: Request): Promise<T> {
  const text = await readLimited(request, 4096);
  if (text === null) throw new HttpError(413, "Request too large.");
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new HttpError(400, "Expected a JSON body.");
  }
}

/** Random bytes as base64url, for tokens, states and one-time codes. */
export function randomToken(bytes = 32): string {
  return base64url(crypto.getRandomValues(new Uint8Array(bytes)));
}

export function base64url(data: Uint8Array): string {
  let text = "";
  for (const byte of data) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function sha256(text: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
}

export { sameText } from "./login";

// No 0/O, 1/I/L: a code gets read out over voice chat.
const CODE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

export function friendCode(): string {
  return randomCode(CODE_ALPHABET, 8);
}

/** "k7qx-m2pd", "K7QX M2PD" → "K7QXM2PD", or null if it cannot be a code. */
export function normalizeCode(text: string): string | null {
  const code = text.replace(/[\s-]/g, "").toUpperCase();
  return code.length === 8 && [...code].every((c) => CODE_ALPHABET.includes(c)) ? code : null;
}

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}
