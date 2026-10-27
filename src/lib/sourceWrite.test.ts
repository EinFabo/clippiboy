import { test } from "node:test";
import assert from "node:assert/strict";
import { removeSourceWrite, sourceWrite } from "./sourceWrite.ts";
import type { AppConfig, AudioSource } from "./types.ts";

const row = (id: string, change: Partial<AudioSource> = {}): AudioSource => ({
  id,
  label: id,
  kind: { type: "game" } as AudioSource["kind"],
  enabled: true,
  gainDb: 0,
  muted: false,
  solo: false,
  separateTrack: false,
  ...change,
});

const at = (revision: number, sources: AudioSource[]) => ({ revision, sources }) as AppConfig;

test("only the fields changed in the window go onto the core's newer copy", async () => {
  const shown = row("mic");
  let sent: { row: AudioSource; revision: number } | null = null;
  const write = sourceWrite({ ...shown, muted: true }, shown, async (r, revision) => {
    sent = { row: r, revision };
    return at(revision + 1, [r]);
  });
  // Turned down elsewhere since the row was built.
  const base = at(7, [row("mic", { gainDb: -6 }), row("game")]);
  const next = write.change(base);
  assert.deepEqual(next.sources[0], row("mic", { gainDb: -6, muted: true }));
  assert.deepEqual(next.sources[1], row("game"));
  await write.send(next, base);
  assert.deepEqual(sent, { row: row("mic", { gainDb: -6, muted: true }), revision: 7 });
});

test("a row removed elsewhere meanwhile is not brought back", async () => {
  const shown = row("mic");
  const write = sourceWrite({ ...shown, muted: true }, shown, async () => {
    throw new Error("sent");
  });
  const base = at(3, [row("game")]);
  const next = write.change(base);
  assert.deepEqual(next.sources, [row("game")]);
  assert.equal(await write.send(next, base), null);
});

test("a new row is added whole", () => {
  const write = sourceWrite(row("mic", { gainDb: 2 }), undefined, async () => null);
  assert.deepEqual(write.change(at(1, [row("game")])).sources, [row("game"), row("mic", { gainDb: 2 })]);
});

test("two edits of one row waiting together keep both", () => {
  const shown = row("mic");
  const mute = sourceWrite({ ...shown, muted: true }, shown, async () => null);
  const down = sourceWrite({ ...shown, muted: true, gainDb: -3 }, { ...shown, muted: true }, async () => null);
  assert.equal(mute.key, down.key);
  const next = down.change(mute.change(at(1, [shown])));
  assert.deepEqual(next.sources[0], row("mic", { muted: true, gainDb: -3 }));
});

test("a removal takes the row off and carries the base revision", async () => {
  let revision = -1;
  const write = removeSourceWrite("mic", async (_id, r) => {
    revision = r;
    return at(r + 1, []);
  });
  const base = at(4, [row("mic"), row("game")]);
  const next = write.change(base);
  assert.deepEqual(next.sources, [row("game")]);
  await write.send(next, base);
  assert.equal(revision, 4);
});
