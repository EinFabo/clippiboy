import { test } from "node:test";
import assert from "node:assert/strict";
import { e2eTarget, LOCAL, PRODUCTION } from "../scripts/target.mjs";

test("the e2e run goes to a local wrangler dev unless told otherwise", () => {
  assert.deepEqual(e2eTarget(undefined, []), { base: LOCAL });
  assert.deepEqual(e2eTarget("", []), { base: LOCAL });
  assert.deepEqual(e2eTarget("http://localhost:8787/", []), { base: "http://localhost:8787" });
  assert.deepEqual(e2eTarget("http://[::1]:8799", []), { base: "http://[::1]:8799" });
});

test("the deployed worker, or any other host, needs --production", () => {
  assert.ok("error" in e2eTarget(PRODUCTION, []));
  assert.ok("error" in e2eTarget("https://example.workers.dev", []));
  // A host that only begins like a local one is not local.
  assert.ok("error" in e2eTarget("http://127.0.0.1.example.com", []));
  assert.deepEqual(e2eTarget(undefined, ["--production"]), { base: PRODUCTION });
  assert.deepEqual(e2eTarget(PRODUCTION, ["--production"]), { base: PRODUCTION });
});

test("something that is not an http URL is refused", () => {
  assert.ok("error" in e2eTarget("api.clippiboy.com", []));
  assert.ok("error" in e2eTarget("ftp://127.0.0.1", []));
});
