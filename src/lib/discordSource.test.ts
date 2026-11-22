import { test } from "node:test";
import assert from "node:assert/strict";
import {
  discordHint,
  discordKind,
  hasDiscordSource,
  isDiscordExe,
  runningDiscord,
} from "./discordSource.ts";
import type { AudioProcess, AudioSource, SourceKind } from "./types.ts";

const proc = (pid: number, exe: string): AudioProcess => ({ pid, name: exe, exe });
const src = (kind: SourceKind, enabled = true): AudioSource => ({
  id: "s",
  label: "s",
  kind,
  enabled,
  gainDb: 0,
  muted: false,
  solo: false,
  separateTrack: true,
});
const app = (exe: string | null, mode: "include" | "exclude" = "include") =>
  src({ type: "process", pid: 1, exe, mode });

test("all three builds count as Discord, in any case", () => {
  assert.ok(isDiscordExe("Discord.exe"));
  assert.ok(isDiscordExe("DISCORDPTB.EXE"));
  assert.ok(isDiscordExe("DiscordCanary.exe"));
  assert.ok(!isDiscordExe("Discord Helper.exe"));
  assert.ok(!isDiscordExe(null));
});

test("the stable build is picked before PTB and Canary", () => {
  const found = runningDiscord([proc(3, "DiscordCanary.exe"), proc(9, "Discord.exe")]);
  assert.equal(found?.pid, 9);
  assert.equal(runningDiscord([proc(3, "DiscordPTB.exe")])?.pid, 3);
  assert.equal(runningDiscord([proc(3, "chrome.exe")]), undefined);
});

test("a Discord source is one that records Discord, switched on or off", () => {
  assert.ok(hasDiscordSource([app("discord.exe")]));
  assert.ok(hasDiscordSource([src({ type: "process", pid: 1, exe: "Discord.exe", mode: "include" }, false)]));
  assert.ok(!hasDiscordSource([app("Discord.exe", "exclude")]));
  assert.ok(!hasDiscordSource([app(null), app("chrome.exe")]));
  assert.ok(!hasDiscordSource([src({ type: "game" })]));
});

test("the source is built like the picker builds it", () =>
  assert.deepEqual(discordKind(proc(42, "Discord.exe")), {
    type: "process",
    pid: 42,
    exe: "Discord.exe",
    mode: "include",
  }));

test("the note offers the track while Discord plays and has none", () =>
  assert.equal(discordHint([], [proc(7, "Discord.exe")]), "add"));

test("with a Discord track the note says where to turn it down", () => {
  assert.equal(discordHint([app("Discord.exe")], [proc(7, "Discord.exe")]), "turnDown");
  assert.equal(discordHint([app("Discord.exe")], []), "turnDown");
});

test("without Discord there is nothing to say", () =>
  assert.equal(discordHint([app("chrome.exe")], [proc(2, "chrome.exe")]), null));
test("no Discord hint of any kind without per-app capture", () => {
  assert.equal(discordHint([app("Discord.exe")], [proc(7, "Discord.exe")], false), null);
});
test("a Discord hint is still offered where per-app capture works", () =>
  assert.equal(discordHint([], [proc(7, "Discord.exe")], true), "add"));

