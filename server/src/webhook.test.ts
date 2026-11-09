import { test } from "node:test";
import assert from "node:assert/strict";
import {
  fresh,
  grantedWebhook,
  guildName,
  isKey,
  isState,
  seal,
  UNKNOWN_GUILD,
  unseal,
  WEBHOOK_TTL_MS,
  webhookLink,
} from "./webhook.ts";

const KEY = "A".repeat(43);
const OTHER_KEY = "B".repeat(43);

function response(webhook: Record<string, unknown> = {}) {
  return {
    access_token: "x",
    scope: "webhook.incoming guilds",
    webhook: {
      id: "347114750880120863",
      token: "kKDdjXa1g9tKNs0-_yOwLyALC9gydEWP6gr9sHabuK1vuofjhQDDnlOclJeRIvYK-pj_",
      guild_id: "290926792226357250",
      channel_id: "345626669224982402",
      url: "https://discord.com/api/webhooks/347114750880120863/kKDd",
      ...webhook,
    },
  };
}

test("a key is 32 bytes of base64url, a state 24", () => {
  assert.ok(isKey(KEY));
  assert.ok(!isKey(KEY.slice(1)));
  assert.ok(!isKey(`${KEY.slice(1)}=`));
  assert.ok(!isKey(undefined));
  assert.ok(isState("a".repeat(32)));
  assert.ok(!isState("a/".repeat(16)));
});

test("a start runs out after ten minutes, and one from the future is refused", () => {
  assert.ok(fresh(1000, 1000 + WEBHOOK_TTL_MS));
  assert.ok(!fresh(1000, 1001 + WEBHOOK_TTL_MS));
  assert.ok(!fresh(10 * 60_000, 0));
});

test("the webhook comes out of the token response with the server's name", () => {
  const granted = grantedWebhook(response(), [
    { id: "1", name: "Other" },
    { id: "290926792226357250", name: "  The \n Squad  " },
  ]);
  assert.deepEqual(granted, {
    id: "347114750880120863",
    token: "kKDdjXa1g9tKNs0-_yOwLyALC9gydEWP6gr9sHabuK1vuofjhQDDnlOclJeRIvYK-pj_",
    guildId: "290926792226357250",
    channelId: "345626669224982402",
    guild: "The Squad",
  });
});

test("only the shapes Discord hands out pass", () => {
  assert.equal(grantedWebhook({ access_token: "x" }, []), null);
  assert.equal(grantedWebhook(null, []), null);
  assert.equal(grantedWebhook(response({ id: "../evil" }), []), null);
  assert.equal(grantedWebhook(response({ token: "a/b" }), []), null);
  assert.equal(grantedWebhook(response({ token: "" }), []), null);
  assert.equal(grantedWebhook(response({ guild_id: 5 }), []), null);
  assert.equal(grantedWebhook(response({ channel_id: undefined }), []), null);
});

test("without the server in the list, or no list, it is 'your server'", () => {
  assert.equal(grantedWebhook(response(), [{ id: "1", name: "Other" }])?.guild, UNKNOWN_GUILD);
  assert.equal(guildName({ error: "no" }, "1"), UNKNOWN_GUILD);
  assert.equal(guildName([{ id: "1", name: "  " }], "1"), UNKNOWN_GUILD);
  assert.equal([...guildName([{ id: "1", name: "x".repeat(300) }], "1")].length, 100);
});

test("the sealed webhook opens only with the app's key", async () => {
  const granted = grantedWebhook(response(), [])!;
  const sealed = await seal(KEY, granted);
  assert.match(sealed, /^[A-Za-z0-9_-]+$/);
  assert.ok(!sealed.includes(granted.token));
  assert.deepEqual(await unseal(KEY, sealed), granted);
  assert.equal(await unseal(OTHER_KEY, sealed), null);
  // A flipped bit is caught by the tag.
  const flipped = sealed.slice(0, 20) + (sealed[20] === "A" ? "B" : "A") + sealed.slice(21);
  assert.equal(await unseal(KEY, flipped), null);
  assert.equal(await unseal(KEY, "short"), null);
});

test("two seals of the same webhook differ (fresh IV)", async () => {
  const granted = grantedWebhook(response(), [])!;
  assert.notEqual(await seal(KEY, granted), await seal(KEY, granted));
});

test("the link carries only the sealed data", () => {
  assert.equal(webhookLink("abc_-"), "clippiboy://webhook?data=abc_-");
});
