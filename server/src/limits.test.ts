import { test } from "node:test";
import assert from "node:assert/strict";
import { addressKey, readLimited } from "./limits.ts";

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

test("an IPv6 address counts as its /64, an IPv4 address as itself", () => {
  assert.equal(addressKey("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
  assert.equal(addressKey("2001:DB8:1:2::9"), "2001:db8:1:2::/64");
  assert.equal(addressKey("2001:db8::1"), "2001:db8:0:0::/64");
  assert.equal(addressKey("203.0.113.7"), "203.0.113.7");
  assert.equal(addressKey("unknown"), "unknown");
  assert.equal(addressKey("::ffff:203.0.113.7"), "203.0.113.7");
  assert.equal(addressKey("not:an:address"), "not:an:address");
});
