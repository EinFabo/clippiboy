import { test } from "node:test";
import assert from "node:assert/strict";
import { addressKey, dialAs, isJsonObject, readLimited, writeBudget } from "./limits.ts";

const req = (body: string | ReadableStream | null, headers: Record<string, string> = {}) =>
  ({ headers: new Headers(headers), body: typeof body === "string" ? new Response(body).body : body });

test("a body over the limit is refused, by its declared length or by what arrives", async () => {
  assert.equal(await readLimited(req("{}"), 4096), "{}");
  assert.equal(await readLimited(req("a".repeat(5000)), 4096), null);
  assert.equal(await readLimited(req("{}", { "content-length": "999999" }), 4096), null);
  const flood = new ReadableStream({ start(c) { for (let i = 0; i < 10; i++) c.enqueue(new Uint8Array(1024)); c.close(); } });
  assert.equal(await readLimited(req(flood), 4096), null);
  assert.equal(await readLimited(req(null), 4096), "");
});

test("an IPv6 address counts as its /48, an IPv4 address as itself", () => {
  assert.equal(addressKey("2001:db8:1:2:3:4:5:6"), "2001:db8:1::/48");
  assert.equal(addressKey("2001:DB8:1:2::9"), "2001:db8:1::/48");
  assert.equal(addressKey("2001:db8::1"), "2001:db8:0::/48");
  // Every /64 of one site is the same key; another site's is not.
  assert.equal(addressKey("2001:db8:1:ffff::1"), addressKey("2001:db8:1:0::1"));
  assert.notEqual(addressKey("2001:db8:2::1"), addressKey("2001:db8:1::1"));
  assert.equal(addressKey("203.0.113.7"), "203.0.113.7");
  assert.equal(addressKey("unknown"), "unknown");
  assert.equal(addressKey("::ffff:203.0.113.7"), "203.0.113.7");
  assert.equal(addressKey("not:an:address"), "not:an:address");
});

test("only a JSON object is a body the routes take apart", () => {
  assert.equal(isJsonObject({}), true);
  assert.equal(isJsonObject({ code: "x" }), true);
  assert.equal(isJsonObject(null), false);
  assert.equal(isJsonObject([]), false);
  assert.equal(isJsonObject("text"), false);
  assert.equal(isJsonObject(5), false);
  assert.equal(isJsonObject(undefined), false);
});

test("the routes that write most count against a tighter limit", () => {
  const id = "00000000-0000-4000-8000-00000000000a";
  assert.equal(writeBudget("POST", "/shares"), "share");
  assert.equal(writeBudget("PUT", "/shares/abcdefghjk/poster"), "share");
  assert.equal(writeBudget("POST", "/webhook/begin"), "connect");
  assert.equal(writeBudget("PATCH", "/me"), "friend");
  assert.equal(writeBudget("DELETE", `/friends/${id}`), "friend");
  assert.equal(writeBudget("POST", `/blocks/${id}`), "friend");
  assert.equal(writeBudget("DELETE", `/blocks/${id}`), "friend");
  // Counted in requestTo, and only when it writes.
  assert.equal(writeBudget("POST", "/friends/request"), null);
  // Needs a request from the other side first.
  assert.equal(writeBudget("POST", `/friends/${id}/accept`), null);
  // Reads, removals of one's own link, sign-out.
  assert.equal(writeBudget("GET", "/shares/quota"), null);
  assert.equal(writeBudget("DELETE", "/shares/abcdefghjk"), null);
  assert.equal(writeBudget("POST", "/auth/logout"), null);
  assert.equal(writeBudget("POST", "/webhook/cancel"), null);
});

// A dial as index.ts makes it: the sign-in, the address's limit, the 429.
class Refused extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}
function dial(signIn: () => Promise<string>, left: number) {
  let counted = 0;
  const result = dialAs(
    signIn,
    async () => ++counted <= left,
    () => new Refused(429, "Too many connections — wait a minute."),
  );
  return { result, counted: () => counted };
}

test("a dial that signs in never counts against its address, however many there were", async () => {
  const d = dial(async () => "user-a", 0);
  assert.equal(await d.result, "user-a");
  assert.equal(d.counted(), 0);
});

test("a dial that fails to sign in counts against its address, and past the limit it is a 429", async () => {
  const signedOut = async (): Promise<string> => {
    throw new Refused(401, "Signed out — please sign in again.");
  };
  const under = dial(signedOut, 1);
  await assert.rejects(under.result, (err: Refused) => err.status === 401);
  assert.equal(under.counted(), 1);
  const over = dial(signedOut, 0);
  await assert.rejects(over.result, (err: Refused) => err.status === 429 && /wait a minute/.test(err.message));
  assert.equal(over.counted(), 1);
});

test("a dial that fails for any other reason is passed on uncounted", async () => {
  const d = dial(async () => {
    throw new Error("D1 is having a bad moment");
  }, 0);
  await assert.rejects(d.result, /bad moment/);
  assert.equal(d.counted(), 0);
});
