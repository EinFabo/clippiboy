import assert from "node:assert/strict";
import test from "node:test";
import { guardViewer, whenViewerFree } from "./viewerGuard.ts";

test("with no viewer open the way out is taken at once", () => {
  let went = 0;
  whenViewerFree(() => went++);
  assert.equal(went, 1);
});

test("an open viewer decides when the way out is taken", () => {
  let held: (() => void) | null = null;
  const release = guardViewer((go) => (held = go));
  let went = 0;
  whenViewerFree(() => went++);
  assert.equal(went, 0);
  held!();
  assert.equal(went, 1);
  release();
  whenViewerFree(() => went++);
  assert.equal(went, 2);
});

test("a viewer that goes only takes its own guard away", () => {
  const asked: string[] = [];
  const first = guardViewer(() => asked.push("first"));
  const second = guardViewer(() => asked.push("second"));
  first();
  whenViewerFree(() => asked.push("went"));
  assert.deepEqual(asked, ["second"]);
  second();
  whenViewerFree(() => asked.push("went"));
  assert.deepEqual(asked, ["second", "went"]);
});
