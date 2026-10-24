import { test } from "node:test";
import assert from "node:assert/strict";
import { autoFit } from "./fit.ts";
import type { CaptureTarget } from "./types.ts";

const screen = (width: number, height: number, refreshHz: number) =>
  ({ width, height, refreshHz }) as CaptureTarget;
const rec = { width: 2560, height: 1440, fps: 165 };

test("no source changes nothing", () => assert.equal(autoFit(rec, null, false), null));
test("a screen that only stands in never changes the saved size", () =>
  assert.equal(autoFit(rec, screen(1920, 1080, 60), true), null));
test("the chosen screen still pulls size and rate down to what it has", () =>
  assert.deepEqual(autoFit(rec, screen(1920, 1080, 60), false), { width: 1920, height: 1080, fps: 60 }));
test("a setting that already fits is left alone", () =>
  assert.equal(autoFit(rec, screen(2560, 1440, 165), false), null));
