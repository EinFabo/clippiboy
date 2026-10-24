import { test } from "node:test";
import assert from "node:assert/strict";
import { cleanPing, cutoffDay, downloadName, isBot, refFromPath, utcDay } from "./telemetry.ts";

const ping = {
  id: "3f2b8c1e-9a4d-4e6f-8b2a-1c3d5e7f9a0b",
  version: "0.8.3",
  source: "tiktok1",
  heard: "discord",
  clips: 3,
  shots: 1,
  recordings: 0,
  features: 5,
};

test("a well-formed ping passes unchanged", () => {
  assert.deepEqual(cleanPing(ping), ping);
});

test("source and heard may be left out", () => {
  const { source: _s, heard: _h, ...rest } = ping;
  assert.deepEqual(cleanPing(rest), { ...rest, source: null, heard: null });
});

test("a malformed ping is turned away", () => {
  const bad: unknown[] = [
    { ...ping, id: "abc" },
    { ...ping, id: "3f2b8c1e-9a4d-1e6f-8b2a-1c3d5e7f9a0b" },
    { ...ping, features: 64 },
    { ...ping, clips: -1 },
    { ...ping, clips: 10001 },
    { ...ping, clips: 1.5 },
    { ...ping, version: "x".repeat(33) },
    { ...ping, source: "Bad!" },
    { ...ping, heard: "myspace" },
    "ping",
    null,
  ];
  for (const raw of bad) assert.equal(cleanPing(raw), null, JSON.stringify(raw));
});

test("days are UTC dates", () => {
  assert.equal(utcDay(Date.UTC(2026, 9, 11, 23, 59)), "2026-10-11");
  assert.equal(cutoffDay(Date.UTC(2026, 9, 11), 400), "2025-09-06");
});

test("the download path names the source, or the site", () => {
  assert.equal(refFromPath("/get/tiktok1"), "tiktok1");
  assert.equal(refFromPath("/get/TikTok1"), "tiktok1");
  assert.equal(refFromPath("/get"), "site");
  assert.equal(refFromPath("/get/"), "site");
  assert.equal(refFromPath("/get/Bad!"), "site");
  assert.equal(refFromPath("/get/" + "a".repeat(33)), "site");
});

test("crawlers are not downloads", () => {
  assert.equal(isBot("Mozilla/5.0 (compatible; Discordbot/2.0)"), true);
  assert.equal(isBot("Googlebot"), true);
  assert.equal(isBot("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/130"), false);
  assert.equal(isBot(null), false);
});

test("link previews are not downloads either", () => {
  assert.equal(isBot("facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)"), true);
  assert.equal(isBot("WhatsApp/2.23.20.0"), true);
  assert.equal(isBot("TelegramBot (like TwitterBot)"), true);
  assert.equal(isBot("Mozilla/5.0 (compatible; Embedly/0.2)"), true);
});

test("the setup carries its source in the file name", () => {
  assert.equal(downloadName("0.8.3", "site"), "ClippiBoy_0.8.3_x64-setup.site.exe");
});

test("only known links count as their own source", () => {
  assert.equal(refFromPath("/get/tiktok1", ["site", "tiktok1"]), "tiktok1");
  assert.equal(refFromPath("/get/made-up", ["site", "tiktok1"]), "site");
  assert.equal(refFromPath("/get/made-up"), "made-up");
});
