import { test } from "node:test";
import assert from "node:assert/strict";
import { discordMb } from "./discord.ts";

test("a Discord copy aims at 95 % of the account's limit", () => {
  assert.equal(discordMb("free"), 19);
  assert.equal(discordMb("nitroBasic"), 47);
  assert.equal(discordMb("nitro"), 475);
});

test("a config from before the setting counts as a free account", () => {
  assert.equal(discordMb(undefined), 19);
});
