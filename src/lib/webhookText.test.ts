import { test } from "node:test";
import assert from "node:assert/strict";
import { postLabel } from "./webhookText.ts";

test("the entry names the server, and says when the clip is on its way", () => {
  assert.equal(postLabel({ guild: "The Squad", posting: [] }, "a"), "Post to The Squad");
  assert.equal(postLabel({ guild: "The Squad", posting: ["b", "a"] }, "a"), "Posting to The Squad…");
  assert.equal(postLabel({ guild: null, posting: [] }, "a"), "Post to Discord");
});
