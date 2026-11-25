// Bounds on requests: how much of a body is read and what it must be, which
// address a rate limit counts against, which dials of the live connection
// count against it, and which signed-in routes count against a tighter limit
// of their own.
//
// No imports, so `node --test` can load this file as it is and the site proxy
// can share it.

/**
 * The body as text, or null when it is over `max` bytes — by its declared
 * length (then nothing is read) or by what actually arrives (then reading
 * stops there). A missing Content-Length is fine: the running count decides.
 */
export async function readLimited(request: Pick<Request, "headers" | "body">, max: number): Promise<string | null> {
  const declared = Number(request.headers.get("Content-Length") ?? "");
  if (Number.isFinite(declared) && declared > max) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const decoder = new TextDecoder();
  let size = 0;
  let text = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel().catch(() => {});
      return null;
    }
    text += decoder.decode(value, { stream: true });
  }
  return text + decoder.decode();
}

/**
 * The key a per-address limit counts against. An IPv6 address counts as its
 * /48 — that is what one site may get, and counted per /64 it held 65,536
 * budgets, enough to spend the database's daily writes through any limit per
 * address. IPv4 and anything unreadable stay as they are.
 */
export function addressKey(address: string): string {
  if (!address.includes(":")) return address;
  const groups = expandIpv6(address.split("%")[0].toLowerCase());
  if (!groups) return address;
  // An IPv4 address in IPv6 dress (::ffff:a.b.c.d) is still that one address.
  if (groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff) {
    return [groups[6] >> 8, groups[6] & 0xff, groups[7] >> 8, groups[7] & 0xff].join(".");
  }
  return `${groups.slice(0, 3).map((g) => g.toString(16)).join(":")}::/48`;
}

/** The eight 16-bit groups, or null if it is no IPv6 address. */
function expandIpv6(address: string): number[] | null {
  const halves = address.split("::");
  if (halves.length > 2) return null;
  const parse = (part: string): number[] | null => {
    if (part === "") return [];
    const out: number[] = [];
    const pieces = part.split(":");
    for (let i = 0; i < pieces.length; i++) {
      const piece = pieces[i];
      if (i === pieces.length - 1 && piece.includes(".")) {
        const parts = piece.split(".");
        if (parts.length !== 4 || !parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255)) return null;
        const [a, b, c, d] = parts.map(Number);
        out.push((a << 8) | b, (c << 8) | d);
      } else if (/^[0-9a-f]{1,4}$/.test(piece)) {
        out.push(parseInt(piece, 16));
      } else {
        return null;
      }
    }
    return out;
  };
  const head = parse(halves[0]);
  const tail = halves.length === 2 ? parse(halves[1]) : [];
  if (!head || !tail) return null;
  if (halves.length === 1) return head.length === 8 ? head : null;
  const missing = 8 - head.length - tail.length;
  if (missing < 1) return null;
  return [...head, ...new Array<number>(missing).fill(0), ...tail];
}

/**
 * The account a dial of the live connection (`GET /ws`) signs in as. Only a
 * dial that fails to sign in (a 401) counts against its address, and past the
 * limit it is answered with `tooMany()` instead. Counted before the sign-in,
 * ten dials a minute from one shared address — a carrier's NAT, an IPv6 /48 —
 * kept everybody behind it offline, and after a deploy everybody behind one
 * address dials back at once. A signed-in dial counts per account (index.ts);
 * one that is not wakes no hub. Any other failure, the database having a bad
 * moment, is passed on uncounted.
 */
export async function dialAs<T>(
  signIn: () => Promise<T>,
  underLimit: () => Promise<boolean>,
  tooMany: () => Error,
): Promise<T> {
  try {
    return await signIn();
  } catch (err) {
    const signedOut = typeof err === "object" && err !== null && (err as { status?: unknown }).status === 401;
    if (signedOut && !(await underLimit())) throw tooMany();
    throw err;
  }
}

/**
 * A JSON body the routes can take apart: an object, not null, an array or a
 * bare value. Destructuring `null` threw, and the request ended in a 500.
 */
export function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The tighter limit a signed-in route counts against on top of the user's
 * general one, or null. These are the routes whose calls write the most for
 * the least: an upload that fails (each try a row and two R2 writes), a poster,
 * the start of a channel connection (a row, deleted again), and friend changes
 * that can be undone and done again. A friend request counts only once it
 * writes something (see `requestTo` in index.ts): a mistyped name must not run
 * into it.
 */
export function writeBudget(method: string, path: string): "share" | "connect" | "friend" | null {
  if (method === "POST" && path === "/shares") return "share";
  if (method === "POST" && path === "/webhook/begin") return "connect";
  if (method === "PUT" && /^\/shares\/[a-z0-9]+\/poster$/.test(path)) return "share";
  if (method === "PATCH" && path === "/me") return "friend";
  if (method === "DELETE" && /^\/friends\/[0-9a-f-]{36}$/.test(path)) return "friend";
  if ((method === "POST" || method === "DELETE") && /^\/blocks\/[0-9a-f-]{36}$/.test(path)) return "friend";
  return null;
}
