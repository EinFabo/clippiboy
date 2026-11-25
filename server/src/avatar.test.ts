import { test } from "node:test";
import assert from "node:assert/strict";
import { avatarRoute, discordAvatar, looksLikePng } from "./avatar.ts";

const hash = "0123456789abcdef0123456789abcdef";

test("the page's picture is ours, keyed by the share, with no Discord id in it", () => {
  const route = avatarRoute("https://api.clippiboy.com", "abcdefghjk");
  assert.equal(route, "https://api.clippiboy.com/c/abcdefghjk/avatar");
  assert.doesNotMatch(route, /discord/);
});

test("the avatars we store are fetched from Discord's CDN at 128 px", () => {
  assert.equal(
    discordAvatar(`https://cdn.discordapp.com/avatars/123456789012345678/${hash}.png?size=128`),
    `https://cdn.discordapp.com/avatars/123456789012345678/${hash}.png?size=128`,
  );
  assert.equal(
    discordAvatar(`https://cdn.discordapp.com/avatars/1/a_${hash}.png`),
    `https://cdn.discordapp.com/avatars/1/a_${hash}.png?size=128`,
  );
  assert.equal(discordAvatar("https://cdn.discordapp.com/embed/avatars/3.png"), "https://cdn.discordapp.com/embed/avatars/3.png?size=128");
});

test("nothing else is fetched", () => {
  for (const stored of [
    null,
    "",
    "not a url",
    `http://cdn.discordapp.com/avatars/1/${hash}.png`,
    `https://cdn.discordapp.com:8443/avatars/1/${hash}.png`,
    `https://cdn.discordapp.com.evil.example/avatars/1/${hash}.png`,
    `https://media.discordapp.net/avatars/1/${hash}.png`,
    `https://cdn.discordapp.com/attachments/1/2/file.png`,
    `https://cdn.discordapp.com/avatars/1/${hash}.gif`,
    `https://cdn.discordapp.com/avatars/1/../../attachments/${hash}.png`,
    "https://cdn.discordapp.com/embed/avatars/12.png",
  ]) {
    assert.equal(discordAvatar(stored), null, String(stored));
  }
});

test("only a PNG passes as one", () => {
  assert.equal(looksLikePng(new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0])), true);
  assert.equal(looksLikePng(new Uint8Array([0xff, 0xd8, 0xff])), false);
  assert.equal(looksLikePng(new TextEncoder().encode("<svg onload=alert(1)>")), false);
});
