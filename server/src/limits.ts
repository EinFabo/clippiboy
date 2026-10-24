// Bounds for the routes anyone may call: how much of a body is read, and which
// address a rate limit counts against.
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
 * /64 — that is what one connection gets, so cycling through it buys no fresh
 * budget. IPv4 and anything unreadable stay as they are.
 */
export function addressKey(address: string): string {
  if (!address.includes(":")) return address;
  const groups = expandIpv6(address.split("%")[0].toLowerCase());
  if (!groups) return address;
  // An IPv4 address in IPv6 dress (::ffff:a.b.c.d) is still that one address.
  if (groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff) {
    return [groups[6] >> 8, groups[6] & 0xff, groups[7] >> 8, groups[7] & 0xff].join(".");
  }
  return `${groups.slice(0, 4).map((g) => g.toString(16)).join(":")}::/64`;
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
