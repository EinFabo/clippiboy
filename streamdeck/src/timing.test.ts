import assert from "node:assert/strict";
import { test } from "node:test";

import { failureOf, keepsConnection, timeoutFor } from "./timing.ts";

test("a status question gives up after two seconds", () => assert.equal(timeoutFor("GET", "/v1/status"), 2000));
test("saving a clip or a picture may take its time", () => {
	assert.equal(timeoutFor("POST", "/v1/clip"), 60_000);
	assert.equal(timeoutFor("POST", "/v1/screenshot"), 30_000);
});
test("toggling the buffer gets fifteen seconds", () => assert.equal(timeoutFor("POST", "/v1/buffer"), 15_000));
test("any other press gets fifteen seconds", () => assert.equal(timeoutFor("POST", "/v1/other"), 15_000));
test("stopping a recording may take ten minutes", () => assert.equal(timeoutFor("POST", "/v1/recording"), 600_000));
test("a timeout means busy, anything else means gone", () => {
	assert.equal(failureOf(new DOMException("t", "TimeoutError")), "slow");
	assert.equal(failureOf(new TypeError("fetch failed")), "gone");
	assert.equal(failureOf(new DOMException("a", "AbortError")), "gone");
	assert.equal(failureOf("nope"), "gone");
});

test("only a real status answer keeps the connection", () => {
	assert.equal(keepsConnection("status", 200, { version: "1", bufferActive: true }), true);
	assert.equal(keepsConnection("status", 200, { hello: 1 }), false);
	assert.equal(keepsConnection("status", 200, null), false);
	assert.equal(keepsConnection("status", 404, null), false);
	assert.equal(keepsConnection("status", 403, null), false);
});
test("a press answer keeps the connection only when it comes from ClippiBoy", () => {
	assert.equal(keepsConnection("press", 200, { ok: true }), true);
	assert.equal(keepsConnection("press", 500, { ok: false, error: "x" }), true);
	assert.equal(keepsConnection("press", 403, { ok: false }), false);
	assert.equal(keepsConnection("press", 404, null), false);
	assert.equal(keepsConnection("press", 200, "<html>"), false);
});
